//! Port of libs/librepcb/core/serialization/serializableobjectlist.h.
//!
//! Differences to upstream:
//! - Elements are owned by value instead of through `std::shared_ptr`;
//!   cloning the list deep-copies the elements, moving it moves them.
//!   Undo commands can take elements out ([`take()`](SerializableObjectList::take))
//!   and put them back ([`insert()`](SerializableObjectList::insert)).
//! - The `onEdited`/`onElementEdited` signals are not ported (yet).
//! - Lookup by pointer identity is replaced by lookup by index.

use std::collections::HashSet;
use std::fmt;
use std::marker::PhantomData;

use super::{DeserializeObject, Error, List, Result, SExpression, SerializeObject};
use crate::types::Uuid;

/// Provides the S-expression tag name of list elements (upstream template
/// parameter `P` with its `tagname` member).
pub trait ListTagName {
    /// Name of the child lists holding the elements, e.g. `"pad"`.
    const TAG_NAME: &'static str;
}

/// Elements identified by a UUID.
pub trait HasUuid {
    /// Returns the UUID of the element.
    fn uuid(&self) -> Uuid;
}

/// Elements identified by a name.
pub trait HasName {
    /// Returns the name of the element.
    fn name(&self) -> &str;
}

/// An ordered list of serializable objects, serialized as one child list
/// named [`P::TAG_NAME`](ListTagName::TAG_NAME) per element.
pub struct SerializableObjectList<T, P> {
    objects: Vec<T>,
    tag: PhantomData<fn() -> P>,
}

impl<T, P: ListTagName> SerializableObjectList<T, P> {
    /// Creates an empty list.
    pub fn new() -> Self {
        Self {
            objects: Vec::new(),
            tag: PhantomData,
        }
    }

    /// Returns the number of elements.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Returns whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Returns the elements as slice.
    pub fn as_slice(&self) -> &[T] {
        &self.objects
    }

    /// Returns an iterator over the elements.
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.objects.iter()
    }

    /// Returns a mutable iterator over the elements.
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.objects.iter_mut()
    }

    /// Returns the element at `index`, if any.
    pub fn get(&self, index: usize) -> Option<&T> {
        self.objects.get(index)
    }

    /// Returns the element at `index` mutably, if any.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        self.objects.get_mut(index)
    }

    /// Returns the first element, if any.
    pub fn first(&self) -> Option<&T> {
        self.objects.first()
    }

    /// Returns the last element, if any.
    pub fn last(&self) -> Option<&T> {
        self.objects.last()
    }

    /// Inserts an element; `index` is clamped to `0..=len()`. Returns the
    /// index of the inserted element.
    pub fn insert(&mut self, index: usize, obj: T) -> usize {
        let index = index.min(self.objects.len());
        self.objects.insert(index, obj);
        index
    }

    /// Appends an element and returns its index.
    pub fn push(&mut self, obj: T) -> usize {
        self.insert(self.objects.len(), obj)
    }

    /// Removes and returns the element at `index`, if it exists.
    pub fn take(&mut self, index: usize) -> Option<T> {
        (index < self.objects.len()).then(|| self.objects.remove(index))
    }

    /// Removes all elements.
    pub fn clear(&mut self) {
        self.objects.clear();
    }

    /// Swaps two elements; both indices are clamped to the valid range.
    pub fn swap(&mut self, i: usize, j: usize) {
        if self.objects.len() < 2 {
            return;
        }
        let max = self.objects.len() - 1;
        self.objects.swap(i.min(max), j.min(max));
    }

    /// Returns a copy of the list sorted with the given comparison function.
    pub fn sorted_by(&self, compare: impl FnMut(&T, &T) -> std::cmp::Ordering) -> Self
    where
        T: Clone,
    {
        let mut objects = self.objects.clone();
        objects.sort_by(compare);
        Self::from(objects)
    }

    /// Consumes the list and returns its elements.
    pub fn into_vec(self) -> Vec<T> {
        self.objects
    }

    /// Replaces the content by the elements deserialized from all children
    /// of `node` named [`P::TAG_NAME`](ListTagName::TAG_NAME). Returns the
    /// number of loaded elements.
    pub fn load_from_sexpression(&mut self, node: &SExpression) -> Result<usize>
    where
        T: DeserializeObject,
    {
        self.objects = node
            .children_named(P::TAG_NAME)
            .map(T::deserialize)
            .collect::<Result<_>>()?;
        Ok(self.objects.len())
    }
}

impl<T: HasUuid, P: ListTagName> SerializableObjectList<T, P> {
    /// Returns the UUIDs of all elements in list order.
    pub fn uuids(&self) -> Vec<Uuid> {
        self.objects.iter().map(HasUuid::uuid).collect()
    }

    /// Returns the set of UUIDs of all elements.
    pub fn uuid_set(&self) -> HashSet<Uuid> {
        self.objects.iter().map(HasUuid::uuid).collect()
    }

    /// Returns the index of the element with the given UUID.
    pub fn index_of_uuid(&self, uuid: &Uuid) -> Option<usize> {
        self.objects.iter().position(|o| o.uuid() == *uuid)
    }

    /// Returns whether an element with the given UUID exists.
    pub fn contains_uuid(&self, uuid: &Uuid) -> bool {
        self.index_of_uuid(uuid).is_some()
    }

    /// Returns the element with the given UUID, if any (upstream `find()`).
    pub fn by_uuid(&self, uuid: &Uuid) -> Option<&T> {
        self.objects.iter().find(|o| o.uuid() == *uuid)
    }

    /// Returns the element with the given UUID mutably, if any.
    pub fn by_uuid_mut(&mut self, uuid: &Uuid) -> Option<&mut T> {
        self.objects.iter_mut().find(|o| o.uuid() == *uuid)
    }

    /// Same as [`by_uuid()`](Self::by_uuid), but returns an error if not
    /// found (upstream `get()`), e.g. for references in files.
    pub fn required_by_uuid(&self, uuid: &Uuid) -> Result<&T> {
        self.by_uuid(uuid).ok_or_else(|| Error::UuidNotFound {
            tag: P::TAG_NAME,
            uuid: uuid.to_string(),
        })
    }

    /// Removes and returns the element with the given UUID, if any.
    pub fn take_by_uuid(&mut self, uuid: &Uuid) -> Option<T> {
        let index = self.index_of_uuid(uuid)?;
        self.take(index)
    }

    /// Returns a copy of the list sorted by UUID.
    pub fn sorted_by_uuid(&self) -> Self
    where
        T: Clone,
    {
        self.sorted_by(|a, b| a.uuid().cmp(&b.uuid()))
    }
}

impl<T: HasName, P: ListTagName> SerializableObjectList<T, P> {
    /// Returns the index of the element with the given name.
    ///
    /// Case insensitive comparison uses Unicode lowercase mapping.
    pub fn index_of_name(&self, name: &str, case_sensitive: bool) -> Option<usize> {
        if case_sensitive {
            self.objects.iter().position(|o| o.name() == name)
        } else {
            let name = name.to_lowercase();
            self.objects
                .iter()
                .position(|o| o.name().to_lowercase() == name)
        }
    }

    /// Returns whether an element with the given name (case sensitive)
    /// exists.
    pub fn contains_name(&self, name: &str) -> bool {
        self.index_of_name(name, true).is_some()
    }

    /// Returns the element with the given name, if any (upstream `find()`).
    pub fn by_name(&self, name: &str, case_sensitive: bool) -> Option<&T> {
        self.index_of_name(name, case_sensitive)
            .map(|i| &self.objects[i])
    }

    /// Same as [`by_name()`](Self::by_name) (case sensitive), but returns an
    /// error if not found (upstream `get()`).
    pub fn required_by_name(&self, name: &str) -> Result<&T> {
        self.by_name(name, true).ok_or_else(|| Error::NameNotFound {
            tag: P::TAG_NAME,
            name: name.to_owned(),
        })
    }

    /// Removes and returns the element with the given name (case sensitive),
    /// if any.
    pub fn take_by_name(&mut self, name: &str) -> Option<T> {
        let index = self.index_of_name(name, true)?;
        self.take(index)
    }
}

impl<T: SerializeObject, P: ListTagName> SerializeObject for SerializableObjectList<T, P> {
    /// Appends one child list per element, each preceded by a line break,
    /// followed by a final line break.
    fn serialize(&self, root: &mut List) {
        for obj in &self.objects {
            root.ensure_line_break();
            obj.serialize(root.append_list(P::TAG_NAME));
        }
        root.ensure_line_break();
    }
}

impl<T: DeserializeObject, P: ListTagName> DeserializeObject for SerializableObjectList<T, P> {
    fn deserialize(node: &SExpression) -> Result<Self> {
        let mut list = Self::new();
        list.load_from_sexpression(node)?;
        Ok(list)
    }
}

impl<T, P: ListTagName> Default for SerializableObjectList<T, P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone, P> Clone for SerializableObjectList<T, P> {
    fn clone(&self) -> Self {
        Self {
            objects: self.objects.clone(),
            tag: PhantomData,
        }
    }
}

impl<T: PartialEq, P> PartialEq for SerializableObjectList<T, P> {
    fn eq(&self, other: &Self) -> bool {
        self.objects == other.objects
    }
}

impl<T: Eq, P> Eq for SerializableObjectList<T, P> {}

impl<T: fmt::Debug, P: ListTagName> fmt::Debug for SerializableObjectList<T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SerializableObjectList")
            .field("tag", &P::TAG_NAME)
            .field("objects", &self.objects)
            .finish()
    }
}

impl<T: serde::Serialize, P> serde::Serialize for SerializableObjectList<T, P> {
    /// Serde: serialized as a plain sequence of the elements.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.objects.serialize(serializer)
    }
}

impl<'de, T: serde::Deserialize<'de>, P> serde::Deserialize<'de> for SerializableObjectList<T, P> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<T>::deserialize(deserializer).map(Self::from)
    }
}

impl<T, P> From<Vec<T>> for SerializableObjectList<T, P> {
    fn from(objects: Vec<T>) -> Self {
        Self {
            objects,
            tag: PhantomData,
        }
    }
}

impl<T, P> FromIterator<T> for SerializableObjectList<T, P> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::from(iter.into_iter().collect::<Vec<_>>())
    }
}

impl<T, P> Extend<T> for SerializableObjectList<T, P> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.objects.extend(iter);
    }
}

impl<T, P> std::ops::Index<usize> for SerializableObjectList<T, P> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        &self.objects[index]
    }
}

impl<T, P> std::ops::IndexMut<usize> for SerializableObjectList<T, P> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        &mut self.objects[index]
    }
}

impl<T, P> IntoIterator for SerializableObjectList<T, P> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.objects.into_iter()
    }
}

impl<'a, T, P> IntoIterator for &'a SerializableObjectList<T, P> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.objects.iter()
    }
}

impl<'a, T, P> IntoIterator for &'a mut SerializableObjectList<T, P> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.objects.iter_mut()
    }
}
