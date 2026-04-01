// Copyright (c) The Diem Core Contributors
// Copyright (c) The Move Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::annotated_value::{
    MoveEnumLayout, MoveFieldLayout, MoveStructLayout, MoveTypeLayout as TreeMoveTypeLayout,
};
use crate::identifier::{IdentStr, Identifier};
use crate::language_storage::{StructTag, TypeTag};
use anyhow::Result as AResult;
use indexmap::IndexSet;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

pub(crate) type Shared<T> = Arc<T>;

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

// =============================================================================
// Compressed (interned) annotated layout types
// =============================================================================

/// Index into an [`MoveTypeLayout`]'s strings table.
pub(crate) type StringIdx = u16;

/// Index into an [`MoveTypeLayout`]'s tags table.
pub(crate) type TagIdx = u16;

/// A list of (field_name_idx, layout_ref) pairs for struct/enum fields.
pub(crate) type AnnotatedFieldIndices = Box<[(StringIdx, LayoutRef)]>;

/// A single variant entry: (variant_name_idx, tag, optional field_indices).
/// `None` field indices means the variant exists but its layout is unknown.
pub(crate) type AnnotatedVariantEntry = (StringIdx, u16, Option<AnnotatedFieldIndices>);

/// Annotated struct layout node: type tag + named fields stored as interned indices.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct MoveStructNode {
    pub(crate) type_: TagIdx,
    pub(crate) fields: AnnotatedFieldIndices,
}

/// Annotated enum layout node: type tag + named variants with named fields.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct MoveEnumNode {
    pub(crate) type_: TagIdx,
    pub(crate) variants: Box<[AnnotatedVariantEntry]>,
}

/// A compound layout node in the annotated compressed node table.
/// Leaf types (primitives) are encoded inline in [`LayoutRef`] and never
/// appear in the table.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum MoveTypeNode {
    Vector(LayoutRef),
    Struct(MoveStructNode),
    Enum(MoveEnumNode),
}

/// The shared pool of interned nodes, strings, and tags backing a [`MoveTypeLayout`].
#[derive(Debug)]
pub struct MoveTypeLayoutPool {
    pub(crate) nodes: Box<[MoveTypeNode]>,
    pub(crate) strings: Box<[Identifier]>,
    pub(crate) tags: Box<[StructTag]>,
}

impl MoveTypeLayoutPool {
    pub fn empty() -> Self {
        MoveTypeLayoutPool {
            nodes: Box::new([]),
            strings: Box::new([]),
            tags: Box::new([]),
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn string_count(&self) -> usize {
        self.strings.len()
    }

    pub fn tag_count(&self) -> usize {
        self.tags.len()
    }
}

/// A deduplicated, flat representation of an annotated [`MoveTypeLayout`] tree.
/// Strings (field names, variant names) and [`StructTag`]s are interned into
/// a shared pool. Cloning is cheap — the pool is shared via `Shared` (Arc).
#[derive(Debug, Clone)]
pub struct MoveTypeLayout {
    pub(crate) pool: Shared<MoveTypeLayoutPool>,
    pub(crate) root: LayoutRef,
}

impl MoveTypeLayout {
    /// Number of compound nodes in the table (excludes inline leaf types).
    pub fn node_count(&self) -> usize {
        self.pool.node_count()
    }

    /// Number of unique interned strings (field/variant names).
    pub fn string_count(&self) -> usize {
        self.pool.string_count()
    }

    /// Number of unique interned struct tags.
    pub fn tag_count(&self) -> usize {
        self.pool.tag_count()
    }

    fn leaf(ty: LeafType) -> Self {
        MoveTypeLayout {
            pool: Shared::new(MoveTypeLayoutPool::empty()),
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

    /// Create a sub-layout rooted at a different position within
    /// the same shared pool. This is a `Shared` bump — no data is copied.
    pub(crate) fn sublayout(&self, root: LayoutRef) -> Self {
        MoveTypeLayout {
            pool: self.pool.clone(),
            root,
        }
    }

    /// Create a resolved view for navigating this layout.
    pub fn as_view(&self) -> MoveLayoutView<'_> {
        resolve_ref(&self.pool, self.root)
    }

    /// If this is a struct, extract a sub-layout for the field at `index`.
    /// The sub-layout shares the same backing pool (cheap `Shared` bump).
    pub fn struct_field_sublayout(&self, index: usize) -> Option<MoveTypeLayout> {
        match self.as_view() {
            MoveLayoutView::Struct(sv) => {
                let (_, field_ref) = sv.fields.raw_field(index)?;
                Some(self.sublayout(field_ref))
            }
            _ => None,
        }
    }

    /// If this is a vector, extract a sub-layout for the element type.
    pub fn vector_element_sublayout(&self) -> Option<MoveTypeLayout> {
        match self.as_view() {
            MoveLayoutView::Vector(vv) => Some(self.sublayout(vv.raw_element())),
            _ => None,
        }
    }

    /// If this is an enum, extract a sub-layout for a variant's field.
    pub fn enum_variant_field_sublayout(
        &self,
        variant_tag: u16,
        field_index: usize,
    ) -> Option<MoveTypeLayout> {
        match self.as_view() {
            MoveLayoutView::Enum(ev) => {
                let (_, vfv) = ev.variant_by_tag(variant_tag)?;
                let fv = match vfv {
                    VariantFieldView::Known(fv) => fv,
                    VariantFieldView::Unknown => return None,
                };
                let (_, field_ref) = fv.raw_field(field_index)?;
                Some(self.sublayout(field_ref))
            }
            _ => None,
        }
    }

    /// Inflate back into a tree-based [`MoveTypeLayout`].
    pub fn inflate(&self) -> AResult<TreeMoveTypeLayout> {
        self.as_view().inflate()
    }
}

/// A compressed layout that is known to be a struct or enum (not a primitive
/// or vector). This mirrors the tree-based [`crate::annotated_value::MoveDatatypeLayout`].
#[derive(Debug, Clone)]
pub struct MoveDatatypeLayout(MoveTypeLayout);

impl MoveDatatypeLayout {
    /// Wrap a `MoveTypeLayout` that is known to be a struct or enum.
    /// Returns `None` if the layout is a primitive or vector.
    pub fn new(layout: MoveTypeLayout) -> Option<Self> {
        match layout.as_view() {
            MoveLayoutView::Struct(_) | MoveLayoutView::Enum(_) => {
                Some(MoveDatatypeLayout(layout))
            }
            _ => None,
        }
    }

    /// Convert into the underlying `MoveTypeLayout`.
    pub fn into_layout(self) -> MoveTypeLayout {
        self.0
    }

    /// Borrow the underlying `MoveTypeLayout`.
    pub fn as_layout(&self) -> &MoveTypeLayout {
        &self.0
    }

    /// Create a view for navigating this layout.
    pub fn as_view(&self) -> MoveLayoutView<'_> {
        self.0.as_view()
    }

    /// Inflate back into a tree-based [`crate::annotated_value::MoveDatatypeLayout`].
    pub fn inflate(&self) -> AResult<crate::annotated_value::MoveDatatypeLayout> {
        let tree = self.0.inflate()?;
        match tree {
            TreeMoveTypeLayout::Struct(s) => {
                Ok(crate::annotated_value::MoveDatatypeLayout::Struct(s))
            }
            TreeMoveTypeLayout::Enum(e) => {
                Ok(crate::annotated_value::MoveDatatypeLayout::Enum(e))
            }
            _ => anyhow::bail!("MoveDatatypeLayout contained non-datatype layout"),
        }
    }
}

// =============================================================================
// View — the primary public API for navigating compressed layouts
// =============================================================================

/// Resolve a [`LayoutRef`] against the pool into a
/// [`MoveLayoutView`] with eagerly resolved type tags and field names.
///
/// Panics if the reference points to an out-of-bounds table index.
pub(crate) fn resolve_ref<'a>(
    pool: &'a MoveTypeLayoutPool,
    r: LayoutRef,
) -> MoveLayoutView<'a> {
    match r.resolve() {
        ResolvedRef::Leaf(leaf) => leaf_to_layout_view(leaf),
        ResolvedRef::Index(idx) => match &pool.nodes[idx] {
            MoveTypeNode::Vector(inner) => MoveLayoutView::Vector(MoveVectorView {
                pool,
                element: *inner,
            }),
            MoveTypeNode::Struct(s) => {
                MoveLayoutView::Struct(MoveStructView {
                    type_: &pool.tags[s.type_ as usize],
                    fields: MoveFieldView {
                        pool,
                        fields: &s.fields,
                    },
                })
            }
            MoveTypeNode::Enum(e) => {
                let type_ = &pool.tags[e.type_ as usize];
                MoveLayoutView::Enum(MoveEnumView {
                    pool,
                    type_,
                    variants: &e.variants,
                })
            }
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

/// A resolved view of an annotated layout node. Compound types contain
/// further views with eagerly resolved type tags and field names.
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
    Struct(MoveStructView<'a>),
    Enum(MoveEnumView<'a>),
}

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
            MoveLayoutView::Struct(sv) => {
                let fields = sv
                    .fields()
                    .map(|(name, fv)| Ok(MoveFieldLayout::new(name.clone(), fv.inflate()?)))
                    .collect::<AResult<_>>()?;
                TreeMoveTypeLayout::Struct(Box::new(MoveStructLayout {
                    type_: sv.type_().clone(),
                    fields,
                }))
            }
            MoveLayoutView::Enum(ev) => {
                let variants = ev
                    .variants()
                    .map(|(variant_name, tag, vfv)| match vfv {
                        VariantFieldView::Known(fv) => {
                            let field_layouts = fv
                                .fields()
                                .map(|(name, fv)| {
                                    Ok(MoveFieldLayout::new(name.clone(), fv.inflate()?))
                                })
                                .collect::<AResult<_>>()?;
                            Ok(((variant_name.clone(), tag), field_layouts))
                        }
                        VariantFieldView::Unknown => {
                            anyhow::bail!("cannot inflate enum with unknown variant layout")
                        }
                    })
                    .collect::<AResult<_>>()?;
                TreeMoveTypeLayout::Enum(Box::new(MoveEnumLayout {
                    type_: ev.type_().clone(),
                    variants,
                }))
            }
        })
    }
}

impl MoveLayoutView<'_> {
    pub fn is_type(&self, t: &TypeTag) -> bool {
        match self {
            MoveLayoutView::Bool => *t == TypeTag::Bool,
            MoveLayoutView::U8 => *t == TypeTag::U8,
            MoveLayoutView::U16 => *t == TypeTag::U16,
            MoveLayoutView::U32 => *t == TypeTag::U32,
            MoveLayoutView::U64 => *t == TypeTag::U64,
            MoveLayoutView::U128 => *t == TypeTag::U128,
            MoveLayoutView::U256 => *t == TypeTag::U256,
            MoveLayoutView::Address => *t == TypeTag::Address,
            MoveLayoutView::Signer => *t == TypeTag::Signer,
            MoveLayoutView::Struct(sv) => sv.is_type(t),
            MoveLayoutView::Vector(vv) => {
                if let TypeTag::Vector(inner) = t {
                    vv.element().is_type(inner)
                } else {
                    false
                }
            }
            MoveLayoutView::Enum(ev) => ev.is_type(t),
        }
    }
}

impl<'a> From<MoveLayoutView<'a>> for TypeTag {
    fn from(view: MoveLayoutView<'a>) -> TypeTag {
        match view {
            MoveLayoutView::Bool => TypeTag::Bool,
            MoveLayoutView::U8 => TypeTag::U8,
            MoveLayoutView::U16 => TypeTag::U16,
            MoveLayoutView::U32 => TypeTag::U32,
            MoveLayoutView::U64 => TypeTag::U64,
            MoveLayoutView::U128 => TypeTag::U128,
            MoveLayoutView::U256 => TypeTag::U256,
            MoveLayoutView::Address => TypeTag::Address,
            MoveLayoutView::Signer => TypeTag::Signer,
            MoveLayoutView::Vector(vv) => {
                TypeTag::Vector(Box::new(TypeTag::from(vv.element())))
            }
            MoveLayoutView::Struct(sv) => {
                TypeTag::Struct(Box::new(sv.type_().clone()))
            }
            MoveLayoutView::Enum(ev) => {
                TypeTag::Struct(Box::new(ev.type_().clone()))
            }
        }
    }
}

/// A lazy view over an annotated vector layout's element type.
#[derive(Debug, Clone, Copy)]
pub struct MoveVectorView<'a> {
    pub(crate) pool: &'a MoveTypeLayoutPool,
    pub(crate) element: LayoutRef,
}

/// A view over a list of named, typed fields (struct fields or enum variant fields).
#[derive(Debug, Clone, Copy)]
pub struct MoveFieldView<'a> {
    pub(crate) pool: &'a MoveTypeLayoutPool,
    pub(crate) fields: &'a [(StringIdx, LayoutRef)],
}

/// A view over an annotated struct layout with type tag and field access.
#[derive(Debug, Clone, Copy)]
pub struct MoveStructView<'a> {
    pub(crate) type_: &'a StructTag,
    pub(crate) fields: MoveFieldView<'a>,
}

/// The result of looking up a variant in an annotated enum view.
#[derive(Debug, Clone, Copy)]
pub enum VariantFieldView<'a> {
    /// The variant's field layout is known.
    Known(MoveFieldView<'a>),
    /// The variant exists but its field layout is not available.
    Unknown,
}

/// A view over an annotated enum layout's variants.
#[derive(Debug, Clone, Copy)]
pub struct MoveEnumView<'a> {
    pub(crate) pool: &'a MoveTypeLayoutPool,
    pub(crate) type_: &'a StructTag,
    pub(crate) variants: &'a [AnnotatedVariantEntry],
}

/// A view over a single named field, mirroring the tree-based [`MoveFieldLayout`].
/// Used by driver accessor methods (`peek_field`, `next_field`, `skip_field`).
#[derive(Debug, Clone, Copy)]
pub struct MoveFieldLayoutView<'a> {
    name: &'a IdentStr,
    layout: MoveLayoutView<'a>,
}

// ---- View impls ----

impl<'a> MoveVectorView<'a> {
    /// Resolve the element type.
    pub fn element(&self) -> MoveLayoutView<'a> {
        resolve_ref(self.pool, self.element)
    }

    /// The raw element ref (for sublayout).
    pub(crate) fn raw_element(&self) -> LayoutRef {
        self.element
    }
}

impl<'a> MoveStructView<'a> {
    /// The struct's type tag.
    pub fn type_(&self) -> &'a StructTag {
        self.type_
    }

    pub fn is_type(&self, t: &TypeTag) -> bool {
        matches!(t, TypeTag::Struct(s) if **s == self.type_().clone())
    }

    /// A field view for iterating/accessing the struct's fields.
    pub fn field_view(&self) -> MoveFieldView<'a> {
        self.fields
    }

    /// Number of fields.
    pub fn field_count(&self) -> usize {
        self.fields.field_count()
    }

    /// Access a field by index, returning `(name, layout_view)`.
    pub fn field(&self, i: usize) -> Option<(&'a Identifier, MoveLayoutView<'a>)> {
        self.fields.field(i)
    }

    /// Iterate over all fields as `(name, layout_view)` pairs.
    pub fn fields(
        &self,
    ) -> impl ExactSizeIterator<Item = (&'a Identifier, MoveLayoutView<'a>)> + 'a {
        let pool = self.fields.pool;
        let fields = self.fields.fields;
        fields.iter().map(move |(name_idx, layout_ref)| {
            (
                &pool.strings[*name_idx as usize],
                resolve_ref(pool, *layout_ref),
            )
        })
    }
}

impl<'a> MoveFieldView<'a> {
    /// Number of fields.
    pub fn field_count(&self) -> usize {
        self.fields.len()
    }

    /// Access a field by index, returning `(name, layout_view)`.
    pub fn field(&self, i: usize) -> Option<(&'a Identifier, MoveLayoutView<'a>)> {
        self.fields.get(i).map(|(name_idx, layout_ref)| {
            (
                &self.pool.strings[*name_idx as usize],
                resolve_ref(self.pool, *layout_ref),
            )
        })
    }

    /// Access a field's raw ref by index (for sublayout).
    pub(crate) fn raw_field(&self, i: usize) -> Option<(&'a Identifier, LayoutRef)> {
        self.fields
            .get(i)
            .map(|(name_idx, layout_ref)| (&self.pool.strings[*name_idx as usize], *layout_ref))
    }

    /// Look up a field by name, returning its layout view.
    pub fn field_by_name(&self, name: &str) -> Option<MoveLayoutView<'a>> {
        self.fields
            .iter()
            .find(|(name_idx, _)| self.pool.strings[*name_idx as usize].as_str() == name)
            .map(|(_, layout_ref)| resolve_ref(self.pool, *layout_ref))
    }

    /// Iterate over all fields as `(name, layout_view)` pairs.
    pub fn fields(
        &self,
    ) -> impl ExactSizeIterator<Item = (&'a Identifier, MoveLayoutView<'a>)> + '_ {
        let pool = self.pool;
        self.fields.iter().map(move |(name_idx, layout_ref)| {
            (
                &pool.strings[*name_idx as usize],
                resolve_ref(pool, *layout_ref),
            )
        })
    }
}

impl<'a> MoveFieldLayoutView<'a> {
    pub fn new(name: &'a IdentStr, layout: MoveLayoutView<'a>) -> Self {
        Self { name, layout }
    }

    pub fn name(&self) -> &'a IdentStr {
        self.name
    }

    pub fn layout(&self) -> MoveLayoutView<'a> {
        self.layout
    }
}

impl<'a> MoveEnumView<'a> {
    /// The enum's type tag.
    pub fn type_(&self) -> &'a StructTag {
        self.type_
    }

    pub fn is_type(&self, t: &TypeTag) -> bool {
        matches!(t, TypeTag::Struct(s) if **s == self.type_().clone())
    }

    /// Number of variants.
    pub fn variant_count(&self) -> usize {
        self.variants.len()
    }

    /// Access a variant by position index. Returns `None` if out of bounds.
    pub fn variant(&self, i: usize) -> Option<(&'a Identifier, u16, VariantFieldView<'a>)> {
        self.variants.get(i).map(|(name_idx, tag, fields)| {
            let name = &self.pool.strings[*name_idx as usize];
            let vfv = match fields {
                Some(fields) => VariantFieldView::Known(MoveFieldView {
                    pool: self.pool,
                    fields,
                }),
                None => VariantFieldView::Unknown,
            };
            (name, *tag, vfv)
        })
    }

    /// Find a variant by its tag value.
    pub fn variant_by_tag(&self, tag: u16) -> Option<(&'a Identifier, VariantFieldView<'a>)> {
        self.variants
            .iter()
            .find(|(_, t, _)| *t == tag)
            .map(|(name_idx, _, fields)| {
                let name = &self.pool.strings[*name_idx as usize];
                let vfv = match fields {
                    Some(fields) => VariantFieldView::Known(MoveFieldView {
                        pool: self.pool,
                        fields,
                    }),
                    None => VariantFieldView::Unknown,
                };
                (name, vfv)
            })
    }

    /// Iterate over all variants as `(name, tag, field_view)` tuples.
    pub fn variants(
        &self,
    ) -> impl ExactSizeIterator<Item = (&'a Identifier, u16, VariantFieldView<'a>)> + 'a
    {
        let pool = self.pool;
        self.variants.iter().map(move |(name_idx, tag, fields)| {
            let name = &pool.strings[*name_idx as usize];
            let vfv = match fields {
                Some(fields) => VariantFieldView::Known(MoveFieldView {
                    pool,
                    fields,
                }),
                None => VariantFieldView::Unknown,
            };
            (name, *tag, vfv)
        })
    }
}

// =============================================================================
// Display — for test debugging
// =============================================================================

impl fmt::Display for MoveLayoutView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MoveLayoutView::Bool => write!(f, "bool"),
            MoveLayoutView::U8 => write!(f, "u8"),
            MoveLayoutView::U16 => write!(f, "u16"),
            MoveLayoutView::U32 => write!(f, "u32"),
            MoveLayoutView::U64 => write!(f, "u64"),
            MoveLayoutView::U128 => write!(f, "u128"),
            MoveLayoutView::U256 => write!(f, "u256"),
            MoveLayoutView::Address => write!(f, "address"),
            MoveLayoutView::Signer => write!(f, "signer"),
            MoveLayoutView::Vector(vv) if f.alternate() => write!(f, "vector<{:#}>", vv.element()),
            MoveLayoutView::Vector(vv) => write!(f, "vector<{}>", vv.element()),
            MoveLayoutView::Struct(sv) if f.alternate() => write!(f, "{sv:#}"),
            MoveLayoutView::Struct(sv) => write!(f, "{sv}"),
            MoveLayoutView::Enum(ev) if f.alternate() => write!(f, "{ev:#}"),
            MoveLayoutView::Enum(ev) => write!(f, "{ev}"),
        }
    }
}

impl fmt::Display for MoveStructView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {{ ", self.type_)?;
        for (i, (name, layout)) in self.fields().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            if f.alternate() {
                write!(f, "{name}: {layout:#}")?;
            } else {
                write!(f, "{name}: {layout}")?;
            }
        }
        write!(f, " }}")
    }
}

impl fmt::Display for MoveEnumView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {{ ", self.type_)?;
        for (i, (variant_name, _tag, vfv)) in self.variants().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{variant_name}(")?;
            match vfv {
                VariantFieldView::Known(fv) => {
                    for (j, (name, layout)) in fv.fields().enumerate() {
                        if j > 0 {
                            write!(f, ", ")?;
                        }
                        if f.alternate() {
                            write!(f, "{name}: {layout:#}")?;
                        } else {
                            write!(f, "{name}: {layout}")?;
                        }
                    }
                }
                VariantFieldView::Unknown => write!(f, "?")?,
            }
            write!(f, ")")?;
        }
        write!(f, " }}")
    }
}

// =============================================================================
// Builder
// =============================================================================

/// Incrementally builds an annotated [`MoveTypeLayout`] with automatic
/// deduplication of nodes, field/variant names, and struct tags.
pub struct MoveTypeLayoutBuilder {
    nodes: IndexSet<MoveTypeNode>,
    strings: IndexSet<Identifier>,
    tags: IndexSet<StructTag>,
}

impl MoveTypeLayoutBuilder {
    pub fn new() -> Self {
        Self {
            nodes: IndexSet::new(),
            strings: IndexSet::new(),
            tags: IndexSet::new(),
        }
    }

    fn intern_string(&mut self, s: &Identifier) -> StringIdx {
        let (idx, _) = self.strings.insert_full(s.clone());
        assert!(
            idx <= u16::MAX as usize,
            "string table exceeds u16 capacity"
        );
        idx as u16
    }

    fn intern_tag(&mut self, tag: &StructTag) -> TagIdx {
        let (idx, _) = self.tags.insert_full(tag.clone());
        assert!(idx <= u16::MAX as usize, "tag table exceeds u16 capacity");
        idx as u16
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

    /// Build a struct layout node.
    /// `fields` is a list of (field_name, field_layout) pairs.
    pub fn struct_layout(
        &mut self,
        type_tag: &StructTag,
        fields: &[(&Identifier, LayoutHandle)],
    ) -> LayoutHandle {
        let tag_idx = self.intern_tag(type_tag);
        let field_indices: AnnotatedFieldIndices = fields
            .iter()
            .map(|(name, h)| (self.intern_string(name), h.0))
            .collect();
        self.intern(MoveTypeNode::Struct(MoveStructNode {
            type_: tag_idx,
            fields: field_indices,
        }))
    }

    /// Build an enum layout node.
    /// Each variant is `(variant_name, tag, fields)` where fields is
    /// `None` for unknown layout or `Some(&[(field_name, layout)])` for known.
    pub fn enum_layout(
        &mut self,
        type_tag: &StructTag,
        variants: &[(&Identifier, u16, Option<&[(&Identifier, LayoutHandle)]>)],
    ) -> LayoutHandle {
        let tag_idx = self.intern_tag(type_tag);
        let variant_entries: Box<[AnnotatedVariantEntry]> = variants
            .iter()
            .map(|(vn, tag, fields)| {
                let vn_idx = self.intern_string(vn);
                let field_indices = fields.map(|fields| {
                    fields
                        .iter()
                        .map(|(fn_name, h)| (self.intern_string(fn_name), h.0))
                        .collect()
                });
                (vn_idx, *tag, field_indices)
            })
            .collect();
        self.intern(MoveTypeNode::Enum(MoveEnumNode {
            type_: tag_idx,
            variants: variant_entries,
        }))
    }

    /// Recursively intern a tree-based annotated layout.
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
                let fields: Vec<(&Identifier, LayoutHandle)> = s
                    .fields
                    .iter()
                    .map(|f| (&f.name, self.intern_tree(&f.layout)))
                    .collect();
                self.struct_layout(&s.type_, &fields)
            }
            TreeMoveTypeLayout::Enum(e) => {
                let variants: Vec<(&Identifier, u16, Vec<(&Identifier, LayoutHandle)>)> = e
                    .variants
                    .iter()
                    .map(|((variant_name, tag), field_layouts)| {
                        let fields: Vec<(&Identifier, LayoutHandle)> = field_layouts
                            .iter()
                            .map(|f| (&f.name, self.intern_tree(&f.layout)))
                            .collect();
                        (variant_name, *tag, fields)
                    })
                    .collect();
                let variant_refs: Vec<(
                    &Identifier,
                    u16,
                    Option<&[(&Identifier, LayoutHandle)]>,
                )> = variants
                    .iter()
                    .map(|(vn, tag, fields)| (*vn, *tag, Some(fields.as_slice())))
                    .collect();
                self.enum_layout(&e.type_, &variant_refs)
            }
        }
    }

    /// Finalize the builder into an immutable [`MoveTypeLayout`].
    pub fn build(self, root: LayoutHandle) -> MoveTypeLayout {
        let nodes: Vec<MoveTypeNode> = self.nodes.into_iter().collect();
        let strings: Vec<Identifier> = self.strings.into_iter().collect();
        let tags: Vec<StructTag> = self.tags.into_iter().collect();
        MoveTypeLayout {
            pool: Shared::new(MoveTypeLayoutPool {
                nodes: nodes.into_boxed_slice(),
                strings: strings.into_boxed_slice(),
                tags: tags.into_boxed_slice(),
            }),
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
