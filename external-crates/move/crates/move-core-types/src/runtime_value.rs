// Copyright (c) The Diem Core Contributors
// Copyright (c) The Move Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::{
    VARIANT_TAG_MAX_VALUE,
    account_address::AccountAddress,
    annotated_value as A, fmt_list,
    runtime_visitor::{Error as VError, ValueDriver, Visitor, visit_struct, visit_value},
    u256,
};
use anyhow::{Result as AResult, anyhow};
use move_proc_macros::test_variant_order;
use serde::{
    Deserialize, Serialize,
    de::Error as DeError,
    ser::{SerializeSeq, SerializeTuple},
};
use std::{
    fmt::{self, Debug},
    io::Cursor,
};

pub use crate::compressed::runtime as compressed_layouts;

/// In the `WithTypes` configuration, a Move struct gets serialized into a Serde struct with this name
pub const MOVE_STRUCT_NAME: &str = "struct";

/// In the `WithTypes` configuration, a Move struct gets serialized into a Serde struct with this as the first field
pub const MOVE_STRUCT_TYPE: &str = "type";

/// In the `WithTypes` configuration, a Move struct gets serialized into a Serde struct with this as the second field
pub const MOVE_STRUCT_FIELDS: &str = "fields";

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct MoveStruct(pub Vec<MoveValue>);

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct MoveVariant {
    pub tag: u16,
    pub fields: Vec<MoveValue>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum MoveValue {
    U8(u8),
    U64(u64),
    U128(u128),
    Bool(bool),
    Address(AccountAddress),
    Vector(Vec<MoveValue>),
    Struct(MoveStruct),
    Signer(AccountAddress),
    // NOTE: Added in bytecode version v6, do not reorder!
    U16(u16),
    U32(u32),
    U256(u256::U256),
    Variant(MoveVariant),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveStructLayout(pub Box<Vec<MoveTypeLayout>>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveEnumLayout(pub Box<Vec<Vec<MoveTypeLayout>>>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MoveDatatypeLayout {
    Struct(Box<MoveStructLayout>),
    Enum(Box<MoveEnumLayout>),
}

impl MoveDatatypeLayout {
    pub fn into_layout(self) -> MoveTypeLayout {
        match self {
            MoveDatatypeLayout::Struct(layout) => MoveTypeLayout::Struct(layout),
            MoveDatatypeLayout::Enum(layout) => MoveTypeLayout::Enum(layout),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[test_variant_order(src/unit_tests/staged_enum_variant_order/move_type_layout.yaml)]
pub enum MoveTypeLayout {
    #[serde(rename(serialize = "bool", deserialize = "bool"))]
    Bool,
    #[serde(rename(serialize = "u8", deserialize = "u8"))]
    U8,
    #[serde(rename(serialize = "u64", deserialize = "u64"))]
    U64,
    #[serde(rename(serialize = "u128", deserialize = "u128"))]
    U128,
    #[serde(rename(serialize = "address", deserialize = "address"))]
    Address,
    #[serde(rename(serialize = "vector", deserialize = "vector"))]
    Vector(Box<MoveTypeLayout>),
    #[serde(rename(serialize = "struct", deserialize = "struct"))]
    Struct(Box<MoveStructLayout>),
    #[serde(rename(serialize = "signer", deserialize = "signer"))]
    Signer,

    // NOTE: Added in bytecode version v6, do not reorder!
    #[serde(rename(serialize = "u16", deserialize = "u16"))]
    U16,
    #[serde(rename(serialize = "u32", deserialize = "u32"))]
    U32,
    #[serde(rename(serialize = "u256", deserialize = "u256"))]
    U256,
    #[serde(rename(serialize = "enum", deserialize = "enum"))]
    Enum(Box<MoveEnumLayout>),
}

impl MoveValue {
    pub fn simple_deserialize(blob: &[u8], ty: &MoveTypeLayout) -> AResult<Self> {
        Ok(bcs::from_bytes_seed(ty, blob)?)
    }

    /// Deserialize a BCS-encoded blob using a compressed type layout.
    pub fn simple_deserialize_compressed(
        blob: &[u8],
        layout: &compressed_layouts::MoveTypeLayout,
    ) -> AResult<Self> {
        Ok(bcs::from_bytes_seed(layout.as_view(), blob)?)
    }

    /// Deserialize `blob` as a Move value with the given `ty`-pe layout, and visit its
    /// sub-structure with the given `visitor`. The visitor dictates the return value that is built
    /// up during deserialization.
    ///
    /// # Nested deserialization
    ///
    /// Vectors and structs are nested structures that can be met during deserialization. Visitors
    /// are passed a driver (`VecDriver` or `StructDriver` correspondingly) which controls how
    /// nested elements or fields are visited including whether a given nested element/field is
    /// explored, which visitor to use (the visitor can pass `self` to recursively explore them) and
    /// whether a given element is visited or skipped.
    ///
    /// The visitor may leave elements unvisited at the end of the vector or struct, which
    /// implicitly skips them.
    ///
    /// # Errors
    ///
    /// Deserialization can fail because of an issue in the serialized format (data doesn't match
    /// layout, unexpected bytes or trailing bytes), or a custom error expressed by the visitor.
    pub fn visit_deserialize<'b, 'l, V: Visitor<'b, 'l>>(
        blob: &'b [u8],
        ty: &'l MoveTypeLayout,
        visitor: &mut V,
    ) -> Result<V::Value, V::Error>
    where
        V::Error: std::error::Error + Send + Sync + 'static,
    {
        let mut bytes = Cursor::new(blob);
        let res = visit_value(&mut bytes, ty, visitor)?;
        if bytes.position() as usize == blob.len() {
            Ok(res)
        } else {
            let remaining = blob.len() - bytes.position() as usize;
            Err(VError::TrailingBytes(remaining).into())
        }
    }

    pub fn simple_serialize(&self) -> Option<Vec<u8>> {
        bcs::to_bytes(self).ok()
    }

    pub fn vector_u8(v: Vec<u8>) -> Self {
        MoveValue::Vector(v.into_iter().map(MoveValue::U8).collect())
    }

    /// Converts the `Vec<MoveValue>` to a `Vec<u8>` if the inner `MoveValue` is a `MoveValue::U8`,
    /// or returns an error otherwise.
    pub fn vec_to_vec_u8(vec: Vec<MoveValue>) -> AResult<Vec<u8>> {
        let mut vec_u8 = Vec::with_capacity(vec.len());

        for byte in vec {
            match byte {
                MoveValue::U8(u8) => {
                    vec_u8.push(u8);
                }
                _ => {
                    return Err(anyhow!(
                        "Expected inner MoveValue in Vec<MoveValue> to be a MoveValue::U8"
                            .to_string(),
                    ));
                }
            }
        }
        Ok(vec_u8)
    }

    pub fn vector_address(v: Vec<AccountAddress>) -> Self {
        MoveValue::Vector(v.into_iter().map(MoveValue::Address).collect())
    }

    pub fn decorate(self, layout: &A::MoveTypeLayout) -> A::MoveValue {
        match (self, layout) {
            (MoveValue::Struct(s), A::MoveTypeLayout::Struct(l)) => {
                A::MoveValue::Struct(s.decorate(l))
            }
            (MoveValue::Variant(s), A::MoveTypeLayout::Enum(l)) => {
                A::MoveValue::Variant(s.decorate(l))
            }
            (MoveValue::Vector(vals), A::MoveTypeLayout::Vector(t)) => {
                A::MoveValue::Vector(vals.into_iter().map(|v| v.decorate(t)).collect())
            }
            (MoveValue::U8(a), _) => A::MoveValue::U8(a),
            (MoveValue::U64(u), _) => A::MoveValue::U64(u),
            (MoveValue::U128(u), _) => A::MoveValue::U128(u),
            (MoveValue::Bool(b), _) => A::MoveValue::Bool(b),
            (MoveValue::Address(a), _) => A::MoveValue::Address(a),
            (MoveValue::Signer(a), _) => A::MoveValue::Signer(a),
            (MoveValue::U16(u), _) => A::MoveValue::U16(u),
            (MoveValue::U32(u), _) => A::MoveValue::U32(u),
            (MoveValue::U256(u), _) => A::MoveValue::U256(u),
            _ => panic!("Invalid decoration"),
        }
    }
}

pub fn serialize_values<'a, I>(vals: I) -> Vec<Vec<u8>>
where
    I: IntoIterator<Item = &'a MoveValue>,
{
    vals.into_iter()
        .map(|val| {
            val.simple_serialize()
                .expect("serialization should succeed")
        })
        .collect()
}

impl MoveStruct {
    pub fn new(value: Vec<MoveValue>) -> Self {
        Self(value)
    }

    pub fn simple_deserialize(blob: &[u8], ty: &MoveStructLayout) -> AResult<Self> {
        Ok(bcs::from_bytes_seed(ty, blob)?)
    }

    /// Like `MoveValue::visit_deserialize` (see for details), but specialized to visiting a struct
    /// (the `blob` is known to be a serialized Move struct, and the layout is a
    /// `MoveStructLayout`).
    pub fn visit_deserialize<'b, 'l, V: Visitor<'b, 'l>>(
        blob: &'b [u8],
        ty: &'l MoveStructLayout,
        visitor: &mut V,
    ) -> Result<V::Value, V::Error>
    where
        V::Error: std::error::Error + Send + Sync + 'static,
    {
        let mut bytes = Cursor::new(blob);
        let driver = ValueDriver::new(&mut bytes, None);
        let res = visit_struct(driver, ty, visitor)?;
        if bytes.position() as usize == blob.len() {
            Ok(res)
        } else {
            let remaining = blob.len() - bytes.position() as usize;
            Err(VError::TrailingBytes(remaining).into())
        }
    }

    pub fn decorate(self, layout: &A::MoveStructLayout) -> A::MoveStruct {
        let MoveStruct(vals) = self;
        let A::MoveStructLayout { type_, fields } = layout;
        A::MoveStruct {
            type_: type_.clone(),
            fields: vals
                .into_iter()
                .zip(fields.iter())
                .map(|(v, l)| (l.name.clone(), v.decorate(&l.layout)))
                .collect(),
        }
    }

    pub fn fields(&self) -> &[MoveValue] {
        &self.0
    }

    pub fn into_fields(self) -> Vec<MoveValue> {
        self.0
    }
}

impl MoveVariant {
    pub fn new(tag: u16, fields: Vec<MoveValue>) -> Self {
        Self { tag, fields }
    }

    pub fn simple_deserialize(blob: &[u8], ty: &MoveEnumLayout) -> AResult<Self> {
        Ok(bcs::from_bytes_seed(ty, blob)?)
    }

    pub fn decorate(self, layout: &A::MoveEnumLayout) -> A::MoveVariant {
        let MoveVariant { tag, fields } = self;
        let A::MoveEnumLayout { type_, variants } = layout;
        let ((v_name, _), v_layout) = variants
            .iter()
            .find(|((_, v_tag), _)| *v_tag == tag)
            .unwrap();
        A::MoveVariant {
            type_: type_.clone(),
            tag,
            fields: fields
                .into_iter()
                .zip(v_layout.iter())
                .map(|(v, l)| (l.name.clone(), v.decorate(&l.layout)))
                .collect(),
            variant_name: v_name.clone(),
        }
    }

    pub fn fields(&self) -> &[MoveValue] {
        &self.fields
    }

    pub fn into_fields(self) -> Vec<MoveValue> {
        self.fields
    }
}

impl MoveStructLayout {
    pub fn new(types: Vec<MoveTypeLayout>) -> Self {
        Self(Box::new(types))
    }

    pub fn fields(&self) -> &[MoveTypeLayout] {
        &self.0
    }

    pub fn field_count(&self) -> usize {
        self.0.len()
    }

    pub fn into_fields(self) -> Vec<MoveTypeLayout> {
        *self.0
    }
}

impl MoveTypeLayout {
    /// Returns a reference to self. Establishes the `as_view()` call pattern
    /// that will later return a compressed `MoveLayoutView` when the type
    /// is swapped to compressed layouts.
    pub fn as_view(&self) -> &Self {
        self
    }
}

impl MoveEnumLayout {
    pub fn variant_count(&self) -> usize {
        self.0.len()
    }
}

impl<'d> serde::de::DeserializeSeed<'d> for &MoveTypeLayout {
    type Value = MoveValue;
    fn deserialize<D: serde::de::Deserializer<'d>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        match self {
            MoveTypeLayout::Bool => bool::deserialize(deserializer).map(MoveValue::Bool),
            MoveTypeLayout::U8 => u8::deserialize(deserializer).map(MoveValue::U8),
            MoveTypeLayout::U16 => u16::deserialize(deserializer).map(MoveValue::U16),
            MoveTypeLayout::U32 => u32::deserialize(deserializer).map(MoveValue::U32),
            MoveTypeLayout::U64 => u64::deserialize(deserializer).map(MoveValue::U64),
            MoveTypeLayout::U128 => u128::deserialize(deserializer).map(MoveValue::U128),
            MoveTypeLayout::U256 => u256::U256::deserialize(deserializer).map(MoveValue::U256),
            MoveTypeLayout::Address => {
                AccountAddress::deserialize(deserializer).map(MoveValue::Address)
            }
            MoveTypeLayout::Signer => {
                AccountAddress::deserialize(deserializer).map(MoveValue::Signer)
            }
            MoveTypeLayout::Struct(ty) => Ok(MoveValue::Struct(ty.deserialize(deserializer)?)),
            MoveTypeLayout::Enum(ty) => Ok(MoveValue::Variant(ty.deserialize(deserializer)?)),
            MoveTypeLayout::Vector(layout) => Ok(MoveValue::Vector(
                deserializer.deserialize_seq(VectorElementVisitor(layout))?,
            )),
        }
    }
}

struct VectorElementVisitor<'a>(&'a MoveTypeLayout);

impl<'d> serde::de::Visitor<'d> for VectorElementVisitor<'_> {
    type Value = Vec<MoveValue>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Vector")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'d>,
    {
        let mut vals = Vec::new();
        while let Some(elem) = seq.next_element_seed(self.0)? {
            vals.push(elem)
        }
        Ok(vals)
    }
}

struct StructFieldVisitor<'a>(&'a [MoveTypeLayout]);

impl<'d> serde::de::Visitor<'d> for StructFieldVisitor<'_> {
    type Value = Vec<MoveValue>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Struct")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'d>,
    {
        let mut val = Vec::new();
        for (i, field_type) in self.0.iter().enumerate() {
            match seq.next_element_seed(field_type)? {
                Some(elem) => val.push(elem),
                None => return Err(A::Error::invalid_length(i, &self)),
            }
        }
        Ok(val)
    }
}

impl<'d> serde::de::DeserializeSeed<'d> for &MoveStructLayout {
    type Value = MoveStruct;

    fn deserialize<D: serde::de::Deserializer<'d>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        Ok(MoveStruct(deserializer.deserialize_tuple(
            self.0.len(),
            StructFieldVisitor(&self.0),
        )?))
    }
}

impl<'d> serde::de::DeserializeSeed<'d> for &MoveEnumLayout {
    type Value = MoveVariant;
    fn deserialize<D: serde::de::Deserializer<'d>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_tuple(2, EnumFieldVisitor(&self.0))
    }
}

struct EnumFieldVisitor<'a>(&'a Vec<Vec<MoveTypeLayout>>);

impl<'d> serde::de::Visitor<'d> for EnumFieldVisitor<'_> {
    type Value = MoveVariant;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Enum")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'d>,
    {
        let tag = match seq.next_element_seed(&MoveTypeLayout::U8)? {
            Some(MoveValue::U8(tag)) if tag as u64 <= VARIANT_TAG_MAX_VALUE => tag as u16,
            Some(MoveValue::U8(tag)) => return Err(A::Error::invalid_length(tag as usize, &self)),
            Some(val) => {
                return Err(A::Error::invalid_type(
                    serde::de::Unexpected::Other(&format!("{val:?}")),
                    &self,
                ));
            }
            None => return Err(A::Error::invalid_length(0, &self)),
        };

        let Some(variant_layout) = self.0.get(tag as usize) else {
            return Err(A::Error::invalid_length(tag as usize, &self));
        };

        let Some(fields) = seq.next_element_seed(&MoveVariantFieldLayout(variant_layout))? else {
            return Err(A::Error::invalid_length(1, &self));
        };

        Ok(MoveVariant { tag, fields })
    }
}

struct MoveVariantFieldLayout<'a>(&'a [MoveTypeLayout]);

impl<'d> serde::de::DeserializeSeed<'d> for &MoveVariantFieldLayout<'_> {
    type Value = Vec<MoveValue>;

    fn deserialize<D: serde::de::Deserializer<'d>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_tuple(self.0.len(), StructFieldVisitor(self.0))
    }
}

impl serde::Serialize for MoveValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            MoveValue::Struct(s) => s.serialize(serializer),
            MoveValue::Variant(v) => v.serialize(serializer),
            MoveValue::Bool(b) => serializer.serialize_bool(*b),
            MoveValue::U8(i) => serializer.serialize_u8(*i),
            MoveValue::U16(i) => serializer.serialize_u16(*i),
            MoveValue::U32(i) => serializer.serialize_u32(*i),
            MoveValue::U64(i) => serializer.serialize_u64(*i),
            MoveValue::U128(i) => serializer.serialize_u128(*i),
            MoveValue::U256(i) => i.serialize(serializer),
            MoveValue::Address(a) => a.serialize(serializer),
            MoveValue::Signer(a) => a.serialize(serializer),
            MoveValue::Vector(v) => {
                let mut t = serializer.serialize_seq(Some(v.len()))?;
                for val in v {
                    t.serialize_element(val)?;
                }
                t.end()
            }
        }
    }
}

impl serde::Serialize for MoveStruct {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut t = serializer.serialize_tuple(self.0.len())?;
        for v in self.0.iter() {
            t.serialize_element(v)?;
        }
        t.end()
    }
}

impl serde::Serialize for MoveVariant {
    // Serialize a variant as:  (tag, [fields...])
    // Since we restrict tags to be less than or equal to 127, the tag will always be a single byte
    // in uleb encoding and we don't actually need to uleb encode it, but we can at a later date if
    // we want/need to.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let tag = if self.tag as u64 > VARIANT_TAG_MAX_VALUE {
            return Err(serde::ser::Error::custom(format!(
                "Variant tag {} is greater than the maximum allowed value of {}",
                self.tag, VARIANT_TAG_MAX_VALUE
            )));
        } else {
            self.tag as u8
        };

        let mut t = serializer.serialize_tuple(2)?;

        t.serialize_element(&tag)?;
        t.serialize_element(&MoveFields(&self.fields))?;

        t.end()
    }
}

struct MoveFields<'a>(&'a [MoveValue]);

impl serde::Serialize for MoveFields<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut t = serializer.serialize_tuple(self.0.len())?;
        for v in self.0.iter() {
            t.serialize_element(v)?;
        }
        t.end()
    }
}

impl fmt::Display for MoveTypeLayout {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        use MoveTypeLayout::*;
        match self {
            Bool => write!(f, "bool"),
            U8 => write!(f, "u8"),
            U16 => write!(f, "u16"),
            U32 => write!(f, "u32"),
            U64 => write!(f, "u64"),
            U128 => write!(f, "u128"),
            U256 => write!(f, "u256"),
            Address => write!(f, "address"),
            Signer => write!(f, "signer"),
            Vector(typ) if f.alternate() => write!(f, "vector<{typ:#}>"),
            Vector(typ) => write!(f, "vector<{typ}>"),
            Struct(s) if f.alternate() => write!(f, "{s:#}"),
            Struct(s) => write!(f, "{s}"),
            Enum(e) if f.alternate() => write!(f, "{e:#}"),
            Enum(e) => write!(f, "{e}"),
        }
    }
}

/// Helper type that uses `T`'s `Display` implementation as its own `Debug` implementation, to allow
/// other `Display` implementations in this module to take advantage of the structured formatting
/// helpers that Rust uses for its own debug types.
struct DebugAsDisplay<'a, T>(&'a T);
impl<T: fmt::Display> fmt::Debug for DebugAsDisplay<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            write!(f, "{:#}", self.0)
        } else {
            write!(f, "{}", self.0)
        }
    }
}

impl fmt::Display for MoveStructLayout {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        use DebugAsDisplay as DD;

        write!(f, "struct ")?;
        let mut map = f.debug_map();
        for (i, l) in self.0.iter().enumerate() {
            map.entry(&i, &DD(&l));
        }

        map.finish()
    }
}

impl fmt::Display for MoveEnumLayout {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        write!(f, "enum ")?;
        for (tag, variant) in self.0.iter().enumerate() {
            write!(f, "variant_tag: {} {{ ", tag)?;
            for (i, l) in variant.iter().enumerate() {
                write!(f, "{}: {}, ", i, l)?
            }
            write!(f, " }} ")?;
        }
        Ok(())
    }
}

impl fmt::Display for MoveValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MoveValue::U8(u) => write!(f, "{}u8", u),
            MoveValue::U16(u) => write!(f, "{}u16", u),
            MoveValue::U32(u) => write!(f, "{}u32", u),
            MoveValue::U64(u) => write!(f, "{}u64", u),
            MoveValue::U128(u) => write!(f, "{}u128", u),
            MoveValue::U256(u) => write!(f, "{}u256", u),
            MoveValue::Bool(false) => write!(f, "false"),
            MoveValue::Bool(true) => write!(f, "true"),
            MoveValue::Address(a) => write!(f, "{}", a.to_hex_literal()),
            MoveValue::Signer(a) => write!(f, "signer({})", a.to_hex_literal()),
            MoveValue::Vector(v) => fmt_list(f, "vector[", v, "]"),
            MoveValue::Struct(s) => fmt::Display::fmt(s, f),
            MoveValue::Variant(v) => fmt::Display::fmt(v, f),
        }
    }
}

impl fmt::Display for MoveStruct {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_list(f, "struct[", &self.0, "]")
    }
}

impl fmt::Display for MoveVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_list(
            f,
            &format!("variant(tag = {})[", self.tag),
            &self.fields,
            "]",
        )
    }
}

pub mod compressed_layouts {
    use super::{MoveEnumLayout, MoveStructLayout, MoveTypeLayout as TreeMoveTypeLayout};
    use indexmap::IndexSet;
    use serde::{Deserialize, Serialize};
    use std::rc::Rc;

    // -------------------------------------------------------------------------
    // LayoutRef — tagged u16 encoding leaf types inline
    // -------------------------------------------------------------------------

    const LEAF_TAG: u16 = 0x8000;

    /// Discriminant for primitive (leaf) Move types, encoded inline in a
    /// [`LayoutRef`] rather than stored in the node table.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[repr(u8)]
    pub(crate) enum LeafType {
        Bool = 0,
        U8 = 1,
        U16 = 2,
        U32 = 3,
        U64 = 4,
        U128 = 5,
        U256 = 6,
        Address = 7,
        Signer = 8,
    }

    impl LeafType {
        fn from_u8(v: u8) -> Option<Self> {
            match v {
                0 => Some(Self::Bool),
                1 => Some(Self::U8),
                2 => Some(Self::U16),
                3 => Some(Self::U32),
                4 => Some(Self::U64),
                5 => Some(Self::U128),
                6 => Some(Self::U256),
                7 => Some(Self::Address),
                8 => Some(Self::Signer),
                _ => None,
            }
        }
    }

    /// A compact reference to a layout node. Bit 15 distinguishes between:
    /// - **Leaf** (bit 15 set): the low bits encode a [`LeafType`] discriminant.
    /// - **Table index** (bit 15 clear): the low 15 bits index into the node table.
    ///
    /// This is an internal storage type. External callers interact with layouts
    /// through [`LayoutHandle`] (for building) and [`MoveLayoutView`] (for reading).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub(crate) struct LayoutRef(u16);

    /// The result of resolving a [`LayoutRef`].
    pub(crate) enum ResolvedRef {
        Leaf(LeafType),
        Index(usize),
    }

    impl LayoutRef {
        pub(crate) const fn leaf(ty: LeafType) -> Self {
            LayoutRef(LEAF_TAG | ty as u16)
        }

        pub(crate) fn index(idx: usize) -> Self {
            assert!(
                idx <= 0x7FFF,
                "table index {idx} exceeds 15-bit maximum (32767)"
            );
            LayoutRef(idx as u16)
        }

        pub(crate) fn resolve(self) -> ResolvedRef {
            if self.0 & LEAF_TAG != 0 {
                let disc = (self.0 & !LEAF_TAG) as u8;
                ResolvedRef::Leaf(
                    LeafType::from_u8(disc)
                        .unwrap_or_else(|| panic!("invalid leaf discriminant: {disc}")),
                )
            } else {
                ResolvedRef::Index(self.0 as usize)
            }
        }
    }

    /// An opaque handle to a layout node returned by the builder.
    ///
    /// Handles are only useful for passing back into the same builder (to compose
    /// compound types) or to [`MoveTypeLayoutBuilder::build`] to designate the root.
    /// The internal representation is not exposed.
    #[derive(Debug, Clone, Copy)]
    pub struct LayoutHandle(pub(crate) LayoutRef);

    // -------------------------------------------------------------------------
    // Node types — only compound nodes stored in the table
    // -------------------------------------------------------------------------

    /// Struct layout node: field types stored as [`LayoutRef`]s.
    #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub(crate) struct MoveStructNode {
        pub(crate) fields: Box<[LayoutRef]>,
    }

    /// Enum layout node: each variant is either a known list of field
    /// [`LayoutRef`]s, or `None` indicating the variant exists but its
    /// field layout is not available.
    #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub(crate) struct MoveEnumNode {
        pub(crate) variants: Box<[Option<Box<[LayoutRef]>>]>,
    }

    /// A compound layout node in the compressed node table.
    /// Leaf types (primitives) are encoded inline in [`LayoutRef`] and never
    /// appear in the table.
    #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub(crate) enum MoveTypeNode {
        Vector(LayoutRef),
        Struct(MoveStructNode),
        Enum(MoveEnumNode),
    }

    /// A deduplicated, flat representation of a [`TreeMoveTypeLayout`] tree.
    /// Cloning is cheap — the node table is shared via `Rc`.
    #[derive(Debug, Clone)]
    pub struct MoveTypeLayout {
        nodes: Rc<[MoveTypeNode]>,
        root: LayoutRef,
    }

    impl MoveTypeLayout {
        /// Number of compound nodes in the table (excludes inline leaf types).
        pub fn node_count(&self) -> usize {
            self.nodes.len()
        }

        fn leaf(ty: LeafType) -> Self {
            MoveTypeLayout {
                nodes: Rc::from([]),
                root: LayoutRef::leaf(ty),
            }
        }

        pub fn bool() -> Self {
            Self::leaf(LeafType::Bool)
        }
        pub fn u8() -> Self {
            Self::leaf(LeafType::U8)
        }
        pub fn u16() -> Self {
            Self::leaf(LeafType::U16)
        }
        pub fn u32() -> Self {
            Self::leaf(LeafType::U32)
        }
        pub fn u64() -> Self {
            Self::leaf(LeafType::U64)
        }
        pub fn u128() -> Self {
            Self::leaf(LeafType::U128)
        }
        pub fn u256() -> Self {
            Self::leaf(LeafType::U256)
        }
        pub fn address() -> Self {
            Self::leaf(LeafType::Address)
        }
        pub fn signer() -> Self {
            Self::leaf(LeafType::Signer)
        }

        /// Create a resolved view for navigating this layout.
        pub fn as_view(&self) -> MoveLayoutView<'_> {
            resolve_ref(&self.nodes, self.root)
        }

        /// Reconstruct the equivalent tree-based layout.
        pub fn inflate(&self) -> AResult<TreeMoveTypeLayout> {
            self.as_view().inflate()
        }
    }

    // -------------------------------------------------------------------------
    // View — the primary public API for navigating compressed layouts
    // -------------------------------------------------------------------------

    /// Resolve a [`LayoutRef`] against the node table into a [`MoveLayoutView`].
    ///
    /// Panics if the reference points to an out-of-bounds table index.
    fn resolve_ref<'a>(nodes: &'a [MoveTypeNode], r: LayoutRef) -> MoveLayoutView<'a> {
        match r.resolve() {
            ResolvedRef::Leaf(leaf) => leaf_to_layout_view(leaf),
            ResolvedRef::Index(idx) => match &nodes[idx] {
                MoveTypeNode::Vector(inner) => MoveLayoutView::Vector(MoveVectorView {
                    nodes,
                    element: *inner,
                }),
                MoveTypeNode::Struct(s) => MoveLayoutView::Struct(MoveFieldView {
                    nodes,
                    fields: &s.fields,
                }),
                MoveTypeNode::Enum(e) => MoveLayoutView::Enum(MoveEnumView {
                    nodes,
                    variants: &e.variants,
                }),
            },
        }
    }

    fn leaf_to_layout_view(leaf: LeafType) -> MoveLayoutView<'static> {
        match leaf {
            LeafType::Bool => MoveLayoutView::Bool,
            LeafType::U8 => MoveLayoutView::U8,
            LeafType::U16 => MoveLayoutView::U16,
            LeafType::U32 => MoveLayoutView::U32,
            LeafType::U64 => MoveLayoutView::U64,
            LeafType::U128 => MoveLayoutView::U128,
            LeafType::U256 => MoveLayoutView::U256,
            LeafType::Address => MoveLayoutView::Address,
            LeafType::Signer => MoveLayoutView::Signer,
        }
    }

    /// A resolved view of a layout node. Leaf types are unit variants;
    /// compound types contain further views for direct navigation.
    /// Resolution is lazy — only one layer is resolved at a time.
    #[derive(Debug, Clone, Copy)]
    pub enum MoveLayoutView<'a> {
        Bool,
        U8,
        U16,
        U32,
        U64,
        U128,
        U256,
        Address,
        Signer,
        Vector(MoveVectorView<'a>),
        Struct(MoveFieldView<'a>),
        Enum(MoveEnumView<'a>),
    }

    use anyhow::Result as AResult;

    impl<'a> MoveLayoutView<'a> {
        /// Reconstruct the equivalent tree-based layout. Returns an error
        /// if any enum variant has an unknown layout.
        pub fn inflate(&self) -> AResult<TreeMoveTypeLayout> {
            Ok(match self {
                MoveLayoutView::Bool => TreeMoveTypeLayout::Bool,
                MoveLayoutView::U8 => TreeMoveTypeLayout::U8,
                MoveLayoutView::U16 => TreeMoveTypeLayout::U16,
                MoveLayoutView::U32 => TreeMoveTypeLayout::U32,
                MoveLayoutView::U64 => TreeMoveTypeLayout::U64,
                MoveLayoutView::U128 => TreeMoveTypeLayout::U128,
                MoveLayoutView::U256 => TreeMoveTypeLayout::U256,
                MoveLayoutView::Address => TreeMoveTypeLayout::Address,
                MoveLayoutView::Signer => TreeMoveTypeLayout::Signer,
                MoveLayoutView::Vector(vv) => {
                    TreeMoveTypeLayout::Vector(Box::new(vv.element().inflate()?))
                }
                MoveLayoutView::Struct(fv) => {
                    let fields = fv.fields().map(|f| f.inflate()).collect::<AResult<_>>()?;
                    TreeMoveTypeLayout::Struct(Box::new(MoveStructLayout::new(fields)))
                }
                MoveLayoutView::Enum(ev) => {
                    let variants = ev
                        .variants()
                        .map(|vfv| match vfv {
                            VariantFieldView::Known(fv) => {
                                fv.fields().map(|f| f.inflate()).collect::<AResult<_>>()
                            }
                            VariantFieldView::Unknown => {
                                anyhow::bail!("cannot inflate enum with unknown variant layout")
                            }
                        })
                        .collect::<AResult<_>>()?;
                    TreeMoveTypeLayout::Enum(Box::new(MoveEnumLayout(Box::new(variants))))
                }
            })
        }
    }

    /// A lazy view over a vector layout's element type.
    #[derive(Debug, Clone, Copy)]
    pub struct MoveVectorView<'a> {
        nodes: &'a [MoveTypeNode],
        element: LayoutRef,
    }

    impl<'a> MoveVectorView<'a> {
        /// Resolve the element type.
        pub fn element(&self) -> MoveLayoutView<'a> {
            resolve_ref(self.nodes, self.element)
        }
    }

    /// A view over a list of typed fields (struct fields or enum variant fields).
    #[derive(Debug, Clone, Copy)]
    pub struct MoveFieldView<'a> {
        nodes: &'a [MoveTypeNode],
        fields: &'a [LayoutRef],
    }

    impl<'a> MoveFieldView<'a> {
        /// Number of fields.
        pub fn field_count(&self) -> usize {
            self.fields.len()
        }

        /// Access a field by index.
        pub fn field(&self, i: usize) -> Option<MoveLayoutView<'a>> {
            self.fields.get(i).map(|&r| resolve_ref(self.nodes, r))
        }

        /// Iterate over all fields as layout views.
        pub fn fields(&self) -> impl ExactSizeIterator<Item = MoveLayoutView<'a>> + '_ {
            let nodes = self.nodes;
            self.fields.iter().map(move |&r| resolve_ref(nodes, r))
        }
    }

    /// The result of looking up a variant in an enum view.
    #[derive(Debug, Clone, Copy)]
    pub enum VariantFieldView<'a> {
        /// The variant's field layout is known.
        Known(MoveFieldView<'a>),
        /// The variant exists but its field layout is not available.
        Unknown,
    }

    /// A view over an enum layout's variants.
    #[derive(Debug, Clone, Copy)]
    pub struct MoveEnumView<'a> {
        nodes: &'a [MoveTypeNode],
        variants: &'a [Option<Box<[LayoutRef]>>],
    }

    impl<'a> MoveEnumView<'a> {
        /// Number of variants.
        pub fn variant_count(&self) -> usize {
            self.variants.len()
        }

        /// Access a variant by index. Returns `None` if out of bounds,
        /// `Some(VariantFieldView::Unknown)` if the variant exists but has
        /// no known layout, or `Some(VariantFieldView::Known(fields))`.
        pub fn variant(&self, i: usize) -> Option<VariantFieldView<'a>> {
            self.variants.get(i).map(|v| match v {
                Some(fields) => VariantFieldView::Known(MoveFieldView {
                    nodes: self.nodes,
                    fields,
                }),
                None => VariantFieldView::Unknown,
            })
        }

        /// Iterate over all variants.
        pub fn variants(&self) -> impl ExactSizeIterator<Item = VariantFieldView<'a>> + 'a {
            let nodes = self.nodes;
            self.variants.iter().map(move |v| match v {
                Some(fields) => VariantFieldView::Known(MoveFieldView { nodes, fields }),
                None => VariantFieldView::Unknown,
            })
        }
    }

    // -------------------------------------------------------------------------
    // Builder
    // -------------------------------------------------------------------------

    /// Incrementally builds a [`MoveTypeLayout`] with automatic deduplication.
    /// Leaf types are encoded inline in [`LayoutRef`] and never stored in the
    /// node table.
    pub struct MoveTypeLayoutBuilder {
        nodes: IndexSet<MoveTypeNode>,
    }

    impl MoveTypeLayoutBuilder {
        pub fn new() -> Self {
            Self {
                nodes: IndexSet::new(),
            }
        }

        fn intern(&mut self, node: MoveTypeNode) -> LayoutHandle {
            let (idx, _) = self.nodes.insert_full(node);
            LayoutHandle(LayoutRef::index(idx))
        }

        pub fn bool(&mut self) -> LayoutHandle {
            LayoutHandle(LayoutRef::leaf(LeafType::Bool))
        }
        pub fn u8(&mut self) -> LayoutHandle {
            LayoutHandle(LayoutRef::leaf(LeafType::U8))
        }
        pub fn u16(&mut self) -> LayoutHandle {
            LayoutHandle(LayoutRef::leaf(LeafType::U16))
        }
        pub fn u32(&mut self) -> LayoutHandle {
            LayoutHandle(LayoutRef::leaf(LeafType::U32))
        }
        pub fn u64(&mut self) -> LayoutHandle {
            LayoutHandle(LayoutRef::leaf(LeafType::U64))
        }
        pub fn u128(&mut self) -> LayoutHandle {
            LayoutHandle(LayoutRef::leaf(LeafType::U128))
        }
        pub fn u256(&mut self) -> LayoutHandle {
            LayoutHandle(LayoutRef::leaf(LeafType::U256))
        }
        pub fn address(&mut self) -> LayoutHandle {
            LayoutHandle(LayoutRef::leaf(LeafType::Address))
        }
        pub fn signer(&mut self) -> LayoutHandle {
            LayoutHandle(LayoutRef::leaf(LeafType::Signer))
        }

        pub fn vector(&mut self, element: LayoutHandle) -> LayoutHandle {
            self.intern(MoveTypeNode::Vector(element.0))
        }

        pub fn struct_layout(&mut self, fields: &[LayoutHandle]) -> LayoutHandle {
            let field_refs: Box<[LayoutRef]> = fields.iter().map(|h| h.0).collect();
            self.intern(MoveTypeNode::Struct(MoveStructNode { fields: field_refs }))
        }

        pub fn enum_layout(&mut self, variants: &[Option<&[LayoutHandle]>]) -> LayoutHandle {
            let variant_refs: Box<[Option<Box<[LayoutRef]>>]> = variants
                .iter()
                .map(|v| v.map(|fields| fields.iter().map(|h| h.0).collect()))
                .collect();
            self.intern(MoveTypeNode::Enum(MoveEnumNode {
                variants: variant_refs,
            }))
        }

        /// Recursively intern a tree-based layout, deduplicating shared subtrees.
        /// Tree-based enum layouts always have known variants, so all variants
        /// are wrapped in `Some`.
        pub fn intern_tree(&mut self, layout: &TreeMoveTypeLayout) -> LayoutHandle {
            match layout {
                TreeMoveTypeLayout::Bool => self.bool(),
                TreeMoveTypeLayout::U8 => self.u8(),
                TreeMoveTypeLayout::U16 => self.u16(),
                TreeMoveTypeLayout::U32 => self.u32(),
                TreeMoveTypeLayout::U64 => self.u64(),
                TreeMoveTypeLayout::U128 => self.u128(),
                TreeMoveTypeLayout::U256 => self.u256(),
                TreeMoveTypeLayout::Address => self.address(),
                TreeMoveTypeLayout::Signer => self.signer(),
                TreeMoveTypeLayout::Vector(inner) => {
                    let inner_h = self.intern_tree(inner);
                    self.vector(inner_h)
                }
                TreeMoveTypeLayout::Struct(s) => {
                    let fields: Vec<LayoutHandle> =
                        s.fields().iter().map(|f| self.intern_tree(f)).collect();
                    self.struct_layout(&fields)
                }
                TreeMoveTypeLayout::Enum(e) => {
                    let variant_handles: Vec<Vec<LayoutHandle>> =
                        e.0.iter()
                            .map(|v| v.iter().map(|f| self.intern_tree(f)).collect())
                            .collect();
                    let variant_refs: Vec<Option<&[LayoutHandle]>> =
                        variant_handles.iter().map(|v| Some(v.as_slice())).collect();
                    self.enum_layout(&variant_refs)
                }
            }
        }

        /// Finalize the builder into an immutable [`MoveTypeLayout`].
        pub fn build(self, root: LayoutHandle) -> MoveTypeLayout {
            let nodes: Vec<MoveTypeNode> = self.nodes.into_iter().collect();
            MoveTypeLayout {
                nodes: Rc::from(nodes),
                root: root.0,
            }
        }
    }

    impl Default for MoveTypeLayoutBuilder {
        fn default() -> Self {
            Self::new()
        }
    }

    impl From<&TreeMoveTypeLayout> for MoveTypeLayout {
        fn from(layout: &TreeMoveTypeLayout) -> Self {
            let mut b = MoveTypeLayoutBuilder::new();
            let root = b.intern_tree(layout);
            b.build(root)
        }
    }

    // -------------------------------------------------------------------------
    // Deserialization — DeserializeSeed for &MoveLayoutView
    // -------------------------------------------------------------------------

    use super::{MoveStruct, MoveValue, MoveVariant};
    use crate::{VARIANT_TAG_MAX_VALUE, account_address::AccountAddress, u256};
    use serde::de::Error as _;

    impl<'d> serde::de::DeserializeSeed<'d> for MoveLayoutView<'_> {
        type Value = MoveValue;

        fn deserialize<D: serde::de::Deserializer<'d>>(
            self,
            deserializer: D,
        ) -> Result<Self::Value, D::Error> {
            match self {
                MoveLayoutView::Bool => bool::deserialize(deserializer).map(MoveValue::Bool),
                MoveLayoutView::U8 => u8::deserialize(deserializer).map(MoveValue::U8),
                MoveLayoutView::U16 => u16::deserialize(deserializer).map(MoveValue::U16),
                MoveLayoutView::U32 => u32::deserialize(deserializer).map(MoveValue::U32),
                MoveLayoutView::U64 => u64::deserialize(deserializer).map(MoveValue::U64),
                MoveLayoutView::U128 => u128::deserialize(deserializer).map(MoveValue::U128),
                MoveLayoutView::U256 => u256::U256::deserialize(deserializer).map(MoveValue::U256),
                MoveLayoutView::Address => {
                    AccountAddress::deserialize(deserializer).map(MoveValue::Address)
                }
                MoveLayoutView::Signer => {
                    AccountAddress::deserialize(deserializer).map(MoveValue::Signer)
                }
                MoveLayoutView::Struct(fv) => {
                    let fields = deserializer
                        .deserialize_tuple(fv.field_count(), CompressedStructFieldVisitor(fv))?;
                    Ok(MoveValue::Struct(MoveStruct(fields)))
                }
                MoveLayoutView::Enum(ev) => {
                    let variant =
                        deserializer.deserialize_tuple(2, CompressedEnumFieldVisitor(ev))?;
                    Ok(MoveValue::Variant(variant))
                }
                MoveLayoutView::Vector(vv) => {
                    let elem = vv.element();
                    Ok(MoveValue::Vector(
                        deserializer.deserialize_seq(CompressedVectorVisitor(elem))?,
                    ))
                }
            }
        }
    }

    struct CompressedVectorVisitor<'a>(MoveLayoutView<'a>);

    impl<'d> serde::de::Visitor<'d> for CompressedVectorVisitor<'_> {
        type Value = Vec<MoveValue>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("Vector")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'d>,
        {
            let mut vals = Vec::new();
            while let Some(elem) = seq.next_element_seed(self.0)? {
                vals.push(elem)
            }
            Ok(vals)
        }
    }

    struct CompressedStructFieldVisitor<'a>(MoveFieldView<'a>);

    impl<'d> serde::de::Visitor<'d> for CompressedStructFieldVisitor<'_> {
        type Value = Vec<MoveValue>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("Struct")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'d>,
        {
            let mut vals = Vec::new();
            for (i, field_view) in self.0.fields().enumerate() {
                match seq.next_element_seed(field_view)? {
                    Some(elem) => vals.push(elem),
                    None => return Err(A::Error::invalid_length(i, &self)),
                }
            }
            Ok(vals)
        }
    }

    struct CompressedEnumFieldVisitor<'a>(MoveEnumView<'a>);

    impl<'d> serde::de::Visitor<'d> for CompressedEnumFieldVisitor<'_> {
        type Value = MoveVariant;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("Enum")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'d>,
        {
            let tag = match seq.next_element::<u8>()? {
                Some(tag) if tag as u64 <= VARIANT_TAG_MAX_VALUE => tag as u16,
                Some(tag) => return Err(A::Error::invalid_length(tag as usize, &self)),
                None => return Err(A::Error::invalid_length(0, &self)),
            };

            let variant_fv = match self.0.variant(tag as usize) {
                Some(VariantFieldView::Known(fv)) => fv,
                Some(VariantFieldView::Unknown) => {
                    return Err(A::Error::custom(format!(
                        "cannot deserialize variant {tag}: layout unknown"
                    )));
                }
                None => return Err(A::Error::invalid_length(tag as usize, &self)),
            };

            let Some(fields) = seq.next_element_seed(CompressedVariantFieldSeed(variant_fv))?
            else {
                return Err(A::Error::invalid_length(1, &self));
            };

            Ok(MoveVariant { tag, fields })
        }
    }

    struct CompressedVariantFieldSeed<'a>(MoveFieldView<'a>);

    impl<'d> serde::de::DeserializeSeed<'d> for CompressedVariantFieldSeed<'_> {
        type Value = Vec<MoveValue>;

        fn deserialize<D: serde::de::Deserializer<'d>>(
            self,
            deserializer: D,
        ) -> Result<Self::Value, D::Error> {
            deserializer
                .deserialize_tuple(self.0.field_count(), CompressedStructFieldVisitor(self.0))
        }
    }
}
