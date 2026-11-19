//! Font roles: retargeting, weight shifts and pass-through of plain families.

use warpui_core::fonts::roles::{create, generation, resolve, retarget, shift, shift_for};
use warpui_core::fonts::{FamilyId, Properties, Weight};

#[test]
fn a_role_resolves_to_its_current_family_and_weight() {
    let role = create(FamilyId(3));
    let medium = Properties::default().weight(Weight::Medium);
    assert_eq!(resolve(role, medium), (FamilyId(3), medium));

    let before = generation();
    assert!(retarget(role, FamilyId(7), shift_for(Weight::Medium)));
    assert!(generation() > before);
    assert_eq!(
        resolve(role, medium),
        (FamilyId(7), Properties::default().weight(Weight::Semibold))
    );
    assert!(!retarget(role, FamilyId(7), 1), "unchanged roles keep caches");
}

#[test]
fn other_families_pass_through() {
    let bold = Properties::default().weight(Weight::Bold);
    assert_eq!(resolve(FamilyId(5), bold), (FamilyId(5), bold));
}

#[test]
fn weight_shifts_stop_at_the_ends() {
    assert_eq!(shift(Weight::Bold, 5), Weight::Black);
    assert_eq!(shift(Weight::Light, -5), Weight::Thin);
    assert_eq!(shift(Weight::Normal, shift_for(Weight::Light)), Weight::Light);
}
