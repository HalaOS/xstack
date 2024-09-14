// This file is generated by rust-protobuf 3.5.1. Do not edit
// .proto file is parsed by protoc 3.19.4
// @generated

// https://github.com/rust-lang/rust-clippy/issues/702
#![allow(unknown_lints)]
#![allow(clippy::all)]

#![allow(unused_attributes)]
#![cfg_attr(rustfmt, rustfmt::skip)]

#![allow(dead_code)]
#![allow(missing_docs)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(trivial_casts)]
#![allow(unused_results)]
#![allow(unused_mut)]

//! Generated file from `DCUtR.proto`

/// Generated files are compatible only with the same version
/// of protobuf runtime.
const _PROTOBUF_VERSION_CHECK: () = ::protobuf::VERSION_3_5_1;

// @@protoc_insertion_point(message:holepunch.pb.HolePunch)
#[derive(PartialEq,Clone,Default,Debug)]
pub struct HolePunch {
    // message fields
    // @@protoc_insertion_point(field:holepunch.pb.HolePunch.type)
    pub type_: ::std::option::Option<::protobuf::EnumOrUnknown<hole_punch::Type>>,
    // @@protoc_insertion_point(field:holepunch.pb.HolePunch.ObsAddrs)
    pub ObsAddrs: ::std::vec::Vec<::std::vec::Vec<u8>>,
    // special fields
    // @@protoc_insertion_point(special_field:holepunch.pb.HolePunch.special_fields)
    pub special_fields: ::protobuf::SpecialFields,
}

impl<'a> ::std::default::Default for &'a HolePunch {
    fn default() -> &'a HolePunch {
        <HolePunch as ::protobuf::Message>::default_instance()
    }
}

impl HolePunch {
    pub fn new() -> HolePunch {
        ::std::default::Default::default()
    }

    // required .holepunch.pb.HolePunch.Type type = 1;

    pub fn type_(&self) -> hole_punch::Type {
        match self.type_ {
            Some(e) => e.enum_value_or(hole_punch::Type::CONNECT),
            None => hole_punch::Type::CONNECT,
        }
    }

    pub fn clear_type_(&mut self) {
        self.type_ = ::std::option::Option::None;
    }

    pub fn has_type(&self) -> bool {
        self.type_.is_some()
    }

    // Param is passed by value, moved
    pub fn set_type(&mut self, v: hole_punch::Type) {
        self.type_ = ::std::option::Option::Some(::protobuf::EnumOrUnknown::new(v));
    }

    fn generated_message_descriptor_data() -> ::protobuf::reflect::GeneratedMessageDescriptorData {
        let mut fields = ::std::vec::Vec::with_capacity(2);
        let mut oneofs = ::std::vec::Vec::with_capacity(0);
        fields.push(::protobuf::reflect::rt::v2::make_option_accessor::<_, _>(
            "type",
            |m: &HolePunch| { &m.type_ },
            |m: &mut HolePunch| { &mut m.type_ },
        ));
        fields.push(::protobuf::reflect::rt::v2::make_vec_simpler_accessor::<_, _>(
            "ObsAddrs",
            |m: &HolePunch| { &m.ObsAddrs },
            |m: &mut HolePunch| { &mut m.ObsAddrs },
        ));
        ::protobuf::reflect::GeneratedMessageDescriptorData::new_2::<HolePunch>(
            "HolePunch",
            fields,
            oneofs,
        )
    }
}

impl ::protobuf::Message for HolePunch {
    const NAME: &'static str = "HolePunch";

    fn is_initialized(&self) -> bool {
        if self.type_.is_none() {
            return false;
        }
        true
    }

    fn merge_from(&mut self, is: &mut ::protobuf::CodedInputStream<'_>) -> ::protobuf::Result<()> {
        while let Some(tag) = is.read_raw_tag_or_eof()? {
            match tag {
                8 => {
                    self.type_ = ::std::option::Option::Some(is.read_enum_or_unknown()?);
                },
                18 => {
                    self.ObsAddrs.push(is.read_bytes()?);
                },
                tag => {
                    ::protobuf::rt::read_unknown_or_skip_group(tag, is, self.special_fields.mut_unknown_fields())?;
                },
            };
        }
        ::std::result::Result::Ok(())
    }

    // Compute sizes of nested messages
    #[allow(unused_variables)]
    fn compute_size(&self) -> u64 {
        let mut my_size = 0;
        if let Some(v) = self.type_ {
            my_size += ::protobuf::rt::int32_size(1, v.value());
        }
        for value in &self.ObsAddrs {
            my_size += ::protobuf::rt::bytes_size(2, &value);
        };
        my_size += ::protobuf::rt::unknown_fields_size(self.special_fields.unknown_fields());
        self.special_fields.cached_size().set(my_size as u32);
        my_size
    }

    fn write_to_with_cached_sizes(&self, os: &mut ::protobuf::CodedOutputStream<'_>) -> ::protobuf::Result<()> {
        if let Some(v) = self.type_ {
            os.write_enum(1, ::protobuf::EnumOrUnknown::value(&v))?;
        }
        for v in &self.ObsAddrs {
            os.write_bytes(2, &v)?;
        };
        os.write_unknown_fields(self.special_fields.unknown_fields())?;
        ::std::result::Result::Ok(())
    }

    fn special_fields(&self) -> &::protobuf::SpecialFields {
        &self.special_fields
    }

    fn mut_special_fields(&mut self) -> &mut ::protobuf::SpecialFields {
        &mut self.special_fields
    }

    fn new() -> HolePunch {
        HolePunch::new()
    }

    fn clear(&mut self) {
        self.type_ = ::std::option::Option::None;
        self.ObsAddrs.clear();
        self.special_fields.clear();
    }

    fn default_instance() -> &'static HolePunch {
        static instance: HolePunch = HolePunch {
            type_: ::std::option::Option::None,
            ObsAddrs: ::std::vec::Vec::new(),
            special_fields: ::protobuf::SpecialFields::new(),
        };
        &instance
    }
}

impl ::protobuf::MessageFull for HolePunch {
    fn descriptor() -> ::protobuf::reflect::MessageDescriptor {
        static descriptor: ::protobuf::rt::Lazy<::protobuf::reflect::MessageDescriptor> = ::protobuf::rt::Lazy::new();
        descriptor.get(|| file_descriptor().message_by_package_relative_name("HolePunch").unwrap()).clone()
    }
}

impl ::std::fmt::Display for HolePunch {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        ::protobuf::text_format::fmt(self, f)
    }
}

impl ::protobuf::reflect::ProtobufValue for HolePunch {
    type RuntimeType = ::protobuf::reflect::rt::RuntimeTypeMessage<Self>;
}

/// Nested message and enums of message `HolePunch`
pub mod hole_punch {
    #[derive(Clone,Copy,PartialEq,Eq,Debug,Hash)]
    // @@protoc_insertion_point(enum:holepunch.pb.HolePunch.Type)
    pub enum Type {
        // @@protoc_insertion_point(enum_value:holepunch.pb.HolePunch.Type.CONNECT)
        CONNECT = 100,
        // @@protoc_insertion_point(enum_value:holepunch.pb.HolePunch.Type.SYNC)
        SYNC = 300,
    }

    impl ::protobuf::Enum for Type {
        const NAME: &'static str = "Type";

        fn value(&self) -> i32 {
            *self as i32
        }

        fn from_i32(value: i32) -> ::std::option::Option<Type> {
            match value {
                100 => ::std::option::Option::Some(Type::CONNECT),
                300 => ::std::option::Option::Some(Type::SYNC),
                _ => ::std::option::Option::None
            }
        }

        fn from_str(str: &str) -> ::std::option::Option<Type> {
            match str {
                "CONNECT" => ::std::option::Option::Some(Type::CONNECT),
                "SYNC" => ::std::option::Option::Some(Type::SYNC),
                _ => ::std::option::Option::None
            }
        }

        const VALUES: &'static [Type] = &[
            Type::CONNECT,
            Type::SYNC,
        ];
    }

    impl ::protobuf::EnumFull for Type {
        fn enum_descriptor() -> ::protobuf::reflect::EnumDescriptor {
            static descriptor: ::protobuf::rt::Lazy<::protobuf::reflect::EnumDescriptor> = ::protobuf::rt::Lazy::new();
            descriptor.get(|| super::file_descriptor().enum_by_package_relative_name("HolePunch.Type").unwrap()).clone()
        }

        fn descriptor(&self) -> ::protobuf::reflect::EnumValueDescriptor {
            let index = match self {
                Type::CONNECT => 0,
                Type::SYNC => 1,
            };
            Self::enum_descriptor().value_by_index(index)
        }
    }

    // Note, `Default` is implemented although default value is not 0
    impl ::std::default::Default for Type {
        fn default() -> Self {
            Type::CONNECT
        }
    }

    impl Type {
        pub(in super) fn generated_enum_descriptor_data() -> ::protobuf::reflect::GeneratedEnumDescriptorData {
            ::protobuf::reflect::GeneratedEnumDescriptorData::new::<Type>("HolePunch.Type")
        }
    }
}

static file_descriptor_proto_data: &'static [u8] = b"\
    \n\x0bDCUtR.proto\x12\x0cholepunch.pb\"y\n\tHolePunch\x120\n\x04type\x18\
    \x01\x20\x02(\x0e2\x1c.holepunch.pb.HolePunch.TypeR\x04type\x12\x1a\n\
    \x08ObsAddrs\x18\x02\x20\x03(\x0cR\x08ObsAddrs\"\x1e\n\x04Type\x12\x0b\n\
    \x07CONNECT\x10d\x12\t\n\x04SYNC\x10\xac\x02\
";

/// `FileDescriptorProto` object which was a source for this generated file
fn file_descriptor_proto() -> &'static ::protobuf::descriptor::FileDescriptorProto {
    static file_descriptor_proto_lazy: ::protobuf::rt::Lazy<::protobuf::descriptor::FileDescriptorProto> = ::protobuf::rt::Lazy::new();
    file_descriptor_proto_lazy.get(|| {
        ::protobuf::Message::parse_from_bytes(file_descriptor_proto_data).unwrap()
    })
}

/// `FileDescriptor` object which allows dynamic access to files
pub fn file_descriptor() -> &'static ::protobuf::reflect::FileDescriptor {
    static generated_file_descriptor_lazy: ::protobuf::rt::Lazy<::protobuf::reflect::GeneratedFileDescriptor> = ::protobuf::rt::Lazy::new();
    static file_descriptor: ::protobuf::rt::Lazy<::protobuf::reflect::FileDescriptor> = ::protobuf::rt::Lazy::new();
    file_descriptor.get(|| {
        let generated_file_descriptor = generated_file_descriptor_lazy.get(|| {
            let mut deps = ::std::vec::Vec::with_capacity(0);
            let mut messages = ::std::vec::Vec::with_capacity(1);
            messages.push(HolePunch::generated_message_descriptor_data());
            let mut enums = ::std::vec::Vec::with_capacity(1);
            enums.push(hole_punch::Type::generated_enum_descriptor_data());
            ::protobuf::reflect::GeneratedFileDescriptor::new_generated(
                file_descriptor_proto(),
                deps,
                messages,
                enums,
            )
        });
        ::protobuf::reflect::FileDescriptor::new_generated_2(generated_file_descriptor)
    })
}