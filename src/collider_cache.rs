use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use rapier3d::na::Vector3;
use rapier3d::prelude::*;

pub struct ColliderCache {
    cache: HashMap<OwnedColliderCacheKey, Collider>,
}

pub trait ColliderCacheKey {
    fn as_borrowed(&self) -> BorrowedColliderCacheKey;
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum OwnedColliderCacheKey {
    ConvexHull(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum BorrowedColliderCacheKey<'a> {
    ConvexHull(&'a str),
}

impl ColliderCacheKey for OwnedColliderCacheKey {
    fn as_borrowed(&self) -> BorrowedColliderCacheKey {
        match self {
            OwnedColliderCacheKey::ConvexHull(name) => BorrowedColliderCacheKey::ConvexHull(name),
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
    fn to_owned(&self) -> OwnedColliderCacheKey {
        match self {
            BorrowedColliderCacheKey::ConvexHull(name) => {
                OwnedColliderCacheKey::ConvexHull(name.to_string())
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

    pub fn get(&mut self, key: BorrowedColliderCacheKey) -> Collider {
        if let Some(collider) = self.cache.get(&key as &dyn ColliderCacheKey) {
            return collider.clone();
        }

        let collider: Collider = match key {
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
                ColliderBuilder::convex_hull(&points).unwrap().into()
            }
        };
        self.cache.insert(key.to_owned(), collider.clone());

        collider
    }
}
