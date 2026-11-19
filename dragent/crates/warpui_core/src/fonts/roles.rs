//! Font roles: a [`FamilyId`] that text keeps for its whole life while the
//! family it draws with, and a weight shift, can change at runtime.
//!
//! An app hands a role to every element of one kind (all interface text,
//! all code) and retargets it when a typography setting changes, so views
//! never re-resolve or re-store fonts. Every font lookup resolves roles
//! first, and [`generation`] tells layout caches when to drop their entries.

use std::sync::atomic::{AtomicU64, Ordering};

use enum_iterator::{all, cardinality};
use parking_lot::RwLock;

use super::{FamilyId, Properties, Weight};

/// Roles live far above the ids platform font databases hand out (they
/// count up from zero), so the two can never meet.
const FIRST_ROLE: usize = usize::MAX / 2;

#[derive(Clone, Copy)]
struct Role {
    family: FamilyId,
    weight_shift: i8,
}

static ROLES: RwLock<Vec<Role>> = RwLock::new(Vec::new());
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// A new role drawing with `family` at its own weights.
pub fn create(family: FamilyId) -> FamilyId {
    let mut roles = ROLES.write();
    roles.push(Role {
        family,
        weight_shift: 0,
    });
    FamilyId(FIRST_ROLE + roles.len() - 1)
}

/// Point `role` at `family`, with every weight moved `weight_shift` steps
/// (one step is Normal → Medium). Returns whether anything changed.
pub fn retarget(role: FamilyId, family: FamilyId, weight_shift: i8) -> bool {
    let mut roles = ROLES.write();
    let Some(slot) = role.0.checked_sub(FIRST_ROLE).and_then(|i| roles.get_mut(i)) else {
        log::warn!("[fonts] {role:?} is not a font role");
        return false;
    };
    if slot.family == family && slot.weight_shift == weight_shift {
        return false;
    }
    *slot = Role {
        family,
        weight_shift,
    };
    GENERATION.fetch_add(1, Ordering::Relaxed);
    true
}

/// The concrete family and properties a lookup should use: roles become
/// their current family and shifted weight, other families are unchanged.
pub fn resolve(family: FamilyId, properties: Properties) -> (FamilyId, Properties) {
    let Some(index) = family.0.checked_sub(FIRST_ROLE) else {
        return (family, properties);
    };
    match ROLES.read().get(index) {
        Some(role) => (
            role.family,
            properties.weight(shift(properties.weight, role.weight_shift)),
        ),
        None => (family, properties),
    }
}

/// Bumped whenever a role changes; text laid out under an older value used
/// different fonts.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// `weight` moved `steps` along Thin … Black, stopping at either end.
pub fn shift(weight: Weight, steps: i8) -> Weight {
    let index = all::<Weight>().position(|w| w == weight).unwrap_or(0) as i64;
    let last = cardinality::<Weight>() as i64 - 1;
    let target = (index + steps as i64).clamp(0, last) as usize;
    all::<Weight>().nth(target).unwrap_or(weight)
}

/// The shift that makes Normal text draw at `weight`.
pub fn shift_for(weight: Weight) -> i8 {
    let position = |w: Weight| all::<Weight>().position(|x| x == w).unwrap_or(0) as i8;
    position(weight) - position(Weight::Normal)
}

