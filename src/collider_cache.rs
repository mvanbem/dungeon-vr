use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use rapier3d::na::Vector3;
use rapier3d::prelude::*;

pub struct ColliderCache {
    cache: HashMap<OwnedColliderCacheKey, ColliderBuilder>,
}

pub trait ColliderCacheKey {
    fn as_borrowed(&self) -> BorrowedColliderCacheKey;
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum OwnedColliderCacheKey {
    ConvexHull(String),
    TriangleMesh(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum BorrowedColliderCacheKey<'a> {
    ConvexHull(&'a str),
    TriangleMesh(&'a str),
}

impl ColliderCacheKey for OwnedColliderCacheKey {
    fn as_borrowed(&self) -> BorrowedColliderCacheKey {
        match self {
            OwnedColliderCacheKey::ConvexHull(name) => BorrowedColliderCacheKey::ConvexHull(name),
            OwnedColliderCacheKey::TriangleMesh(name) => {
                BorrowedColliderCacheKey::TriangleMesh(name)
            }
        }
    }
}

impl<'a> ColliderCacheKey for BorrowedColliderCacheKey<'a> {
    fn as_borrowed(&self) -> BorrowedColliderCacheKey {
        *self
    }
}

impl<'a> Borrow<dyn ColliderCacheKey + 'a> for OwnedColliderCacheKey {
    fn borrow(&self) -> &(dyn ColliderCacheKey + 'a) {
        self
    }
}

impl<'a> Borrow<dyn ColliderCacheKey + 'a> for BorrowedColliderCacheKey<'a> {
    fn borrow(&self) -> &(dyn ColliderCacheKey + 'a) {
        self
    }
}

impl PartialEq for dyn ColliderCacheKey + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.as_borrowed() == other.as_borrowed()
    }
}

impl Eq for dyn ColliderCacheKey + '_ {}

impl Hash for dyn ColliderCacheKey + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_borrowed().hash(state);
    }
}

impl<'a> BorrowedColliderCacheKey<'a> {
    fn to_owned(self) -> OwnedColliderCacheKey {
        match self {
            BorrowedColliderCacheKey::ConvexHull(name) => {
                OwnedColliderCacheKey::ConvexHull(name.to_string())
            }
            BorrowedColliderCacheKey::TriangleMesh(name) => {
                OwnedColliderCacheKey::TriangleMesh(name.to_string())
            }
        }
    }
}

impl ColliderCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    pub fn get(&mut self, key: BorrowedColliderCacheKey) -> ColliderBuilder {
        if let Some(collider) = self.cache.get(&key as &dyn ColliderCacheKey) {
            return collider.clone();
        }

        let collider = match key {
            BorrowedColliderCacheKey::ConvexHull(name) => {
                let path = format!("assets/{name}.gltf");
                let (document, buffers, _) = gltf::import(&path).unwrap();
                assert_eq!(document.meshes().len(), 1);
                let mesh = document.meshes().next().unwrap();
                let mut points = Vec::new();
                for primitive in mesh.primitives() {
                    let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));
                    for position in reader.read_positions().unwrap() {
                        points.push(nalgebra::OPoint {
                            coords: Vector3::from(position),
                        });
                    }
                }
                ColliderBuilder::convex_hull(&points).unwrap()
            }
            BorrowedColliderCacheKey::TriangleMesh(name) => {
                let path = format!("assets/{name}.gltf");
                let (document, buffers, _) = gltf::import(&path).unwrap();
                assert_eq!(document.meshes().len(), 1);
                let mesh = document.meshes().next().unwrap();
                let mut vertices = Vec::new();
                let mut indices = Vec::new();
                for primitive in mesh.primitives() {
                    assert_eq!(primitive.mode(), gltf::mesh::Mode::Triangles);
                    let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));
                    let base_vertex = vertices.len() as u32;
                    for position in reader.read_positions().unwrap() {
                        vertices.push(nalgebra::OPoint {
                            coords: Vector3::from(position),
                        });
                    }
                    for_each_triangle(reader.read_indices().unwrap(), |[a, b, c]| {
                        indices.push([base_vertex + a, base_vertex + b, base_vertex + c]);
                    });
                }
                ColliderBuilder::trimesh(vertices, indices)
            }
        };
        self.cache.insert(key.to_owned(), collider.clone());

        collider
    }
}

fn for_each_triangle(indices: gltf::mesh::util::ReadIndices, mut f: impl FnMut([u32; 3])) {
    match indices {
        gltf::mesh::util::ReadIndices::U8(mut iter) => {
            while let Some(a) = iter.next() {
                let b = iter.next().unwrap();
                let c = iter.next().unwrap();
                f([a as u32, b as u32, c as u32]);
            }
        }
        gltf::mesh::util::ReadIndices::U16(mut iter) => {
            while let Some(a) = iter.next() {
                let b = iter.next().unwrap();
                let c = iter.next().unwrap();
                f([a as u32, b as u32, c as u32]);
            }
        }
        gltf::mesh::util::ReadIndices::U32(mut iter) => {
            while let Some(a) = iter.next() {
                let b = iter.next().unwrap();
                let c = iter.next().unwrap();
                f([a, b, c]);
            }
        }
    }
}
