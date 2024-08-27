//! Utilities for x509 certificate of libp2p

use std::{
    str::FromStr,
    time::{Duration, SystemTime},
};

use ::sec1::{EcParameters, EcPrivateKey};
use const_oid::{
    db::rfc5912::{
        ECDSA_WITH_SHA_256, ECDSA_WITH_SHA_384, ECDSA_WITH_SHA_512, ID_RSASSA_PSS, ID_SHA_256,
        ID_SHA_384, ID_SHA_512, SHA_256_WITH_RSA_ENCRYPTION, SHA_384_WITH_RSA_ENCRYPTION,
        SHA_512_WITH_RSA_ENCRYPTION,
    },
    AssociatedOid, ObjectIdentifier,
};
use der::{asn1::OctetString, Decode, Encode, Sequence};
use identity::PublicKey;
use p256::elliptic_curve::{sec1::FromEncodedPoint, CurveArithmetic};
use p256::{
    ecdsa::{signature::Verifier, VerifyingKey},
    elliptic_curve::{
        sec1::{ModulusSize, ToEncodedPoint},
        AffinePoint, FieldBytesSize, SecretKey,
    },
};
use rand::{rngs::OsRng, thread_rng, Rng};
use xstack::keystore::KeyStore;
use rsa::pkcs1::RsaPssParams;
use sha2::{digest::FixedOutputReset, Digest};
use x509_cert::{
    builder::{Builder, CertificateBuilder, Profile},
    ext::{AsExtension, Extension},
    name::Name,
    serial_number::SerialNumber,
    spki::{DecodePublicKey, EncodePublicKey, SubjectPublicKeyInfoOwned},
    time::Validity,
    Certificate,
};
use zeroize::Zeroizing;

use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    IoError(#[from] io::Error),

    #[error(transparent)]
    SpkiError(#[from] x509_cert::spki::Error),

    #[error(transparent)]
    X509BuilderError(#[from] x509_cert::builder::Error),

    #[error(transparent)]
    DerError(#[from] x509_cert::der::Error),

    #[error("The received is not a valid libp2p tls handshake certificate: {0}")]
    Libp2pCert(String),

    #[error(transparent)]
    Pkcs1Error(#[from] pkcs1::Error),

    #[error(transparent)]
    Pkcs8Error(#[from] p256::pkcs8::Error),

    #[error(transparent)]
    EllipticCurve(#[from] p256::elliptic_curve::Error),

    #[error(transparent)]
    EcdsaSig(#[from] p256::ecdsa::signature::Error),

    #[error(transparent)]
    DecodingErr(#[from] identity::DecodingError),
}

impl From<Error> for io::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::IoError(io_error) => io_error,
            _ => io::Error::new(io::ErrorKind::Other, value),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// The peer signs the concatenation of the string `libp2p-tls-handshake:`
/// and the public key that it used to generate the certificate carrying
/// the libp2p Public Key Extension, using its private host key.
/// This signature provides cryptographic proof that the peer was
/// in possession of the private host key at the time the certificate was signed.
static P2P_SIGNING_PREFIX: [u8; 21] = *b"libp2p-tls-handshake:";

const P2P_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.4.1.53594.1.1");

/// The public host key and the signature are ANS.1-encoded
/// into the SignedKey data structure, which is carried
/// in the libp2p Public Key Extension.
/// SignedKey ::= SEQUENCE {
///    publicKey OCTET STRING,
///    signature OCTET STRING
/// }
#[derive(Sequence)]
pub struct Libp2pExtension {
    public_key: OctetString,
    signature: OctetString,
}

impl AssociatedOid for Libp2pExtension {
    /// The libp2p Public Key Extension is a X.509 extension
    /// with the Object Identifier 1.3.6.1.4.1.53594.1.1,
    /// allocated by IANA to the libp2p project at Protocol Labs.

    const OID: ObjectIdentifier = P2P_OID;
}

impl AsExtension for Libp2pExtension {
    fn critical(&self, _subject: &x509_cert::name::Name, _extensions: &[Extension]) -> bool {
        true
    }
}

impl Libp2pExtension {
    /// Create a libp2p public key extension with host `keypair` and public key used to generate the certifacte.
    pub async fn new<PubKey: AsRef<[u8]>>(
        keypair: &KeyStore,
        cert_pub_key: PubKey,
    ) -> Result<Self> {
        // The peer signs the concatenation of the string `libp2p-tls-handshake:`
        // and the public key that it used to generate the certificate carrying
        // the libp2p Public Key Extension, using its private host key.
        let signature = {
            let mut msg = vec![];
            msg.extend(P2P_SIGNING_PREFIX);
            msg.extend(cert_pub_key.as_ref());

            keypair.sign(&msg).await?
        };

        let public_key = keypair.public_key().await?.encode_protobuf();

        Ok(Self {
            public_key: OctetString::new(public_key)?,
            signature: OctetString::new(signature)?,
        })
    }

    /// Verify the libp2p self-signed certificate.
    ///
    /// On success, returns [`PeerId`](xstack::identity::PeerId) derived from host public key.
    pub fn verify<PubKey: AsRef<[u8]>>(&self, cert_pub_key: PubKey) -> Result<PublicKey> {
        let mut msg = vec![];
        msg.extend(P2P_SIGNING_PREFIX);
        msg.extend(cert_pub_key.as_ref());

        let pub_key = identity::PublicKey::try_decode_protobuf(self.public_key.as_bytes())?;

        if !pub_key.verify(&msg, self.signature.as_bytes()) {
            return Err(Error::Libp2pCert(
                "Verify libp2p public key extension failed.".into(),
            ));
        }

        Ok(pub_key)
    }
}

/// In order to be able to use arbitrary key types, peers don’t use their host key to sign
/// the X.509 certificate they send during the handshake. Instead, the host key is encoded
/// into the libp2p Public Key Extension, which is carried in a self-signed certificate.
///
/// The key used to generate and sign this certificate SHOULD NOT be related to the host's key.
/// Endpoints MAY generate a new key and certificate for every connection attempt,
/// or they MAY reuse the same key and certificate for multiple connections.
///
/// The `keypair` is the host key provider.
pub async fn generate(keypair: &KeyStore) -> Result<(Vec<u8>, Zeroizing<Vec<u8>>)> {
    let signer = p256::SecretKey::random(&mut OsRng);

    let public_key = signer.public_key();

    let cert_keypair = to_sec1_der(&signer)?;

    let signer = p256::ecdsa::SigningKey::from(signer);

    let serial_number = SerialNumber::from(thread_rng().gen::<u64>());
    let validity = Validity::from_now(Duration::new(5, 0)).unwrap();
    let profile = Profile::Manual { issuer: None };
    let subject = Name::from_str("CN=libp2p domination corporation,O=libp2p domination Inc,C=US")
        .unwrap()
        .to_der()
        .unwrap();
    let subject = Name::from_der(&subject).unwrap();
    let pub_key = SubjectPublicKeyInfoOwned::from_key(public_key)?;

    let mut builder =
        CertificateBuilder::new(profile, serial_number, validity, subject, pub_key, &signer)?;

    let libp2p_extension =
        Libp2pExtension::new(keypair, public_key.to_public_key_der()?.as_bytes()).await?;

    builder.add_extension(&libp2p_extension)?;

    let certifacte = builder.build::<p256::ecdsa::DerSignature>()?;

    Ok((certifacte.to_der()?, cert_keypair))
}

/// Parse and verify the libp2p certificate from ASN.1 DER format.
///
/// On success, returns the [`PeerId`](xstack::identity::PeerId) extract from [`libp2p public key extension`](https://github.com/libp2p/specs/blob/master/tls/tls.md)
pub fn verify<D: AsRef<[u8]>>(der: D) -> Result<PublicKey> {
    let cert = Certificate::from_der(der.as_ref())?;

    validity(&cert)?;

    verify_signature(&cert)?;

    let extension = extract_libp2p_extension(&cert)?;

    let cert_pub_key = cert.tbs_certificate.subject_public_key_info.to_der()?;

    Ok(extension.verify(cert_pub_key)?)
}

// Certificates MUST use the NamedCurve encoding for elliptic curve parameters.
// Similarly, hash functions with an output length less than 256 bits
// MUST NOT be used, due to the possibility of collision attacks.
// In particular, MD5 and SHA1 MUST NOT be used.
// Endpoints MUST abort the connection attempt if it is not used.
pub fn verify_signature(cert: &Certificate) -> Result<()> {
    match cert.signature_algorithm.oid {
        ECDSA_WITH_SHA_256 => verify_ecsda_with_sha256_signature(cert),
        ECDSA_WITH_SHA_384 => verify_ecsda_with_sha384_signature(cert),
        ECDSA_WITH_SHA_512 => verify_ecsda_with_sha521_signature(cert),
        SHA_256_WITH_RSA_ENCRYPTION => verify_rsa_pkcs1_signature::<sha2::Sha256>(cert),
        SHA_384_WITH_RSA_ENCRYPTION => verify_rsa_pkcs1_signature::<sha2::Sha384>(cert),
        SHA_512_WITH_RSA_ENCRYPTION => verify_rsa_pkcs1_signature::<sha2::Sha512>(cert),
        ID_RSASSA_PSS => {
            let params = cert
                .signature_algorithm
                .parameters
                .as_ref()
                .ok_or(Error::Libp2pCert(
                    "Invalid signature algorithm parameters".into(),
                ))?
                .decode_as::<RsaPssParams>()?;

            match params.hash.oid {
                ID_SHA_256 => verify_rsa_pss_signature::<sha2::Sha256>(cert),
                ID_SHA_384 => verify_rsa_pss_signature::<sha2::Sha384>(cert),
                ID_SHA_512 => verify_rsa_pss_signature::<sha2::Sha512>(cert),
                hash_oid => Err(Error::Libp2pCert(format!(
                    "forbidden signature({}) parameter({})",
                    cert.signature_algorithm.oid, hash_oid
                ))),
            }
        }
        oid => Err(Error::Libp2pCert(format!("forbidden signature({})", oid))),
    }
}

/// This function fixes a bug where the output of [`SecretKey::to_sec1_der`]
/// could not be loaded by [`boring::ec::EcKey::private_key_from_der`].
fn to_sec1_der<C>(key: &SecretKey<C>) -> der::Result<Zeroizing<Vec<u8>>>
where
    C: CurveArithmetic + AssociatedOid,
    AffinePoint<C>: FromEncodedPoint<C> + ToEncodedPoint<C>,
    FieldBytesSize<C>: ModulusSize,
{
    let private_key_bytes = Zeroizing::new(key.to_bytes());
    let public_key_bytes = key.public_key().to_encoded_point(false);

    let ec_private_key = Zeroizing::new(
        EcPrivateKey {
            private_key: &private_key_bytes,
            parameters: Some(EcParameters::NamedCurve(C::OID)),
            public_key: Some(public_key_bytes.as_bytes()),
        }
        .to_der()?,
    );

    Ok(ec_private_key)
}

fn verify_rsa_pkcs1_signature<D>(cert: &Certificate) -> Result<()>
where
    D: Digest + AssociatedOid,
{
    let input = cert.tbs_certificate.to_der()?;

    let pub_key = cert.tbs_certificate.subject_public_key_info.to_der()?;

    let verify_key = rsa::pkcs1v15::VerifyingKey::<D>::from_public_key_der(&pub_key)?;

    let signature = rsa::pkcs1v15::Signature::try_from(cert.signature.as_bytes().unwrap_or(&[]))?;

    verify_key.verify(&input, &signature)?;

    Ok(())
}

fn verify_rsa_pss_signature<D>(cert: &Certificate) -> Result<()>
where
    D: Digest + AssociatedOid + FixedOutputReset,
{
    let input = cert.tbs_certificate.to_der()?;

    let pub_key = cert.tbs_certificate.subject_public_key_info.to_der()?;

    println!("verify_rsa_pss_signature: {}", D::OID);

    let verify_key =
        rsa::pss::VerifyingKey::<D>::new(rsa::RsaPublicKey::from_public_key_der(&pub_key)?);

    let signature = rsa::pss::Signature::try_from(cert.signature.as_bytes().unwrap_or(&[]))?;

    verify_key.verify(&input, &signature)?;

    Ok(())
}

fn verify_ecsda_with_sha256_signature(cert: &Certificate) -> Result<()> {
    let input = cert.tbs_certificate.to_der()?;

    let pub_key = cert.tbs_certificate.subject_public_key_info.to_der()?;

    let verify_key = VerifyingKey::from_public_key_der(&pub_key)?;

    let signature =
        p256::ecdsa::DerSignature::from_bytes(cert.signature.as_bytes().unwrap_or(&[]))?;

    verify_key.verify(&input, &signature)?;

    Ok(())
}

fn verify_ecsda_with_sha384_signature(cert: &Certificate) -> Result<()> {
    let input = cert.tbs_certificate.to_der()?;

    let pub_key = cert.tbs_certificate.subject_public_key_info.to_der()?;

    let verify_key = p384::ecdsa::VerifyingKey::from_public_key_der(&pub_key)?;

    let signature =
        p384::ecdsa::DerSignature::from_bytes(cert.signature.as_bytes().unwrap_or(&[]))?;

    verify_key.verify(&input, &signature)?;

    Ok(())
}

fn verify_ecsda_with_sha521_signature(cert: &Certificate) -> Result<()> {
    let input = cert.tbs_certificate.to_der()?;

    let pub_key = cert.tbs_certificate.subject_public_key_info.to_der()?;

    let verify_key = p521::ecdsa::VerifyingKey::from_sec1_bytes(
        &p521::PublicKey::from_public_key_der(&pub_key)?.to_sec1_bytes(),
    )?;

    let signature =
        p521::ecdsa::DerSignature::from_bytes(cert.signature.as_bytes().unwrap_or(&[]))?;

    verify_key.verify(&input, &signature.try_into()?)?;

    Ok(())
}

// be valid at the time it is received by the peer;
fn validity(cert: &Certificate) -> Result<()> {
    let not_after = cert.tbs_certificate.validity.not_after.to_system_time();
    let not_before = cert.tbs_certificate.validity.not_before.to_system_time();

    let now = SystemTime::now();

    if not_after < now || now < not_before {
        return Err(Error::Libp2pCert(format!(
            "Valid time error, not_after={:?}, not_before={:?}, now={:?}",
            not_after, not_before, now
        )));
    }

    Ok(())
}

fn extract_libp2p_extension(cert: &Certificate) -> Result<Libp2pExtension> {
    let extensions = match &cert.tbs_certificate.extensions {
        Some(extension) => extension,
        None => {
            return Err(Error::Libp2pCert(
                "libp2p public key extension not found".into(),
            ))
        }
    };

    let mut libp2p_extension = None;

    for ext in extensions {
        // found p2p extension.
        if ext.extn_id == P2P_OID {
            if libp2p_extension.is_some() {
                return Err(Error::Libp2pCert(
                    "duplicate libp2p public key extension".into(),
                ));
            }

            libp2p_extension = Some(Libp2pExtension::from_der(ext.extn_value.as_bytes())?);

            continue;
        }

        if ext.critical {
            return Err(Error::Libp2pCert(format!(
                "Unknown critical: {}",
                ext.extn_id
            )));
        }
    }

    let libp2p_extension = libp2p_extension.ok_or(Error::Libp2pCert(
        "libp2p public key extension not found".into(),
    ))?;

    Ok(libp2p_extension)
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_verify_signature() {
        fn check(buf: &[u8]) {
            let cert = Certificate::from_der(buf).unwrap();

            verify_signature(&cert).unwrap();
        }

        fn check_failed(buf: &[u8]) {
            let cert = Certificate::from_der(buf).unwrap();

            verify_signature(&cert).expect_err("must failed");
        }

        check(include_bytes!("./x509_testdata/rsa_pkcs1_sha256.der"));
        check(include_bytes!("./x509_testdata/rsa_pkcs1_sha384.der"));
        check(include_bytes!("./x509_testdata/rsa_pkcs1_sha512.der"));
        check(include_bytes!("./x509_testdata/nistp256_sha256.der"));

        check_failed(include_bytes!("./x509_testdata/nistp384_sha256.der"));

        check(include_bytes!("./x509_testdata/nistp384_sha384.der"));
        check(include_bytes!("./x509_testdata/nistp521_sha512.der"));
        // check(include_bytes!("./x509_testdata/rsa_pss_sha256.der"));
    }
}
