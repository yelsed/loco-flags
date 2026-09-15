//! Which side of a rollout a subject falls on.
//!
//! The whole of a percentage rollout is this one function. Nothing is stored: given the same flag
//! and the same subject it answers the same thing on every process, on every machine, for ever,
//! which is what lets there be no third table holding remembered answers.

use sha2::{Digest, Sha256};

/// Where a subject sits on a flag's nought-to-ninety-nine line.
///
/// **sha256 rather than [`std::collections::hash_map::DefaultHasher`]**, which is the obvious
/// reach and the wrong one: its output is explicitly allowed to change between Rust releases. A
/// rollout built on it would reshuffle on a toolchain bump, quietly moving people into a feature
/// and others out of it, and nothing would report that it had happened. sha2 is already in loco's
/// dependency tree, so the guarantee costs no new dependency.
///
/// The flag's key is part of the digest so that two flags at ten percent do not pick the same ten
/// percent of people. Without it, the unluckiest tenth of your users would meet every experiment
/// you ever run.
#[must_use]
pub fn of(key: &str, scope_type: &str, scope_id: &str) -> u8 {
    let digest = Sha256::digest(format!("{key}:{scope_type}:{scope_id}").as_bytes());
    let leading: [u8; 8] = digest[..8]
        .try_into()
        .expect("a sha256 digest is 32 bytes, so its first 8 are always there");
    u8::try_from(u64::from_be_bytes(leading) % 100).expect("a remainder of 100 fits in a u8")
}

/// Whether a subject is inside a rollout of this size.
///
/// Strictly less than, so `0` lets nobody in and `100` lets everybody in. Both ends matter: a
/// rollout at nought is how a flag is prepared without being live, and one at a hundred is how it
/// is finished without being deleted.
#[must_use]
pub fn is_inside(key: &str, scope_type: &str, scope_id: &str, percent: u8) -> bool {
    of(key, scope_type, scope_id) < percent
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number this whole file exists to make true.
    #[test]
    fn a_tenth_of_ten_thousand_people_is_about_a_tenth() {
        let inside = (0..10_000)
            .filter(|subject| is_inside("occasions", "host", &subject.to_string(), 10))
            .count();

        assert!(
            (900..=1100).contains(&inside),
            "ten percent of ten thousand landed at {inside}, which is outside the tolerance a \
             rollout is allowed"
        );
    }

    /// **Raising a percentage may never take the feature away from somebody who had it.**
    ///
    /// This is the property that makes a rollout a rollout rather than a reshuffle. It falls out of
    /// comparing one fixed number against a rising threshold, and it is pinned because any future
    /// cleverness here, salting per run or mixing the percentage into the digest, would break it
    /// without breaking anything that looks like a test.
    #[test]
    fn turning_a_rollout_up_never_puts_anybody_out() {
        for subject in 0..2_000 {
            let id = subject.to_string();
            let joined_at = (0..=100u8)
                .find(|percent| is_inside("occasions", "host", &id, *percent))
                .expect("everybody is inside at a hundred");

            for percent in joined_at..=100 {
                assert!(
                    is_inside("occasions", "host", &id, percent),
                    "host {id} was inside at {joined_at} and fell out again at {percent}"
                );
            }
        }
    }

    /// Two flags at the same percentage must not choose the same people, or one unlucky tenth of
    /// an audience meets every experiment there is.
    #[test]
    fn two_flags_at_one_percentage_pick_different_people() {
        let occasions: Vec<bool> = (0..2_000)
            .map(|subject| is_inside("occasions", "host", &subject.to_string(), 10))
            .collect();
        let paywall: Vec<bool> = (0..2_000)
            .map(|subject| is_inside("paywall", "host", &subject.to_string(), 10))
            .collect();

        assert_ne!(
            occasions, paywall,
            "two flags picked exactly the same audience, so the key is not reaching the digest"
        );
    }

    /// Nought lets nobody in and a hundred lets everybody in, which is what makes those two
    /// numbers usable as "prepared" and "finished".
    #[test]
    fn the_two_ends_mean_what_they_say() {
        for subject in 0..500 {
            let id = subject.to_string();
            assert!(!is_inside("occasions", "host", &id, 0));
            assert!(is_inside("occasions", "host", &id, 100));
        }
    }

    /// The digest is over the three parts joined, so the same identifier under a different scope
    /// type is a different subject.
    #[test]
    fn a_scope_type_is_part_of_who_you_are() {
        let as_host = of("occasions", "host", "42");
        let as_party = of("occasions", "party", "42");
        assert_ne!(as_host, as_party);
    }

    /// Pinned by value. If this number ever changes, every rollout in every deployment reshuffles,
    /// so it must be impossible to do by accident.
    #[test]
    fn the_bucket_is_a_fixed_number_for_ever() {
        assert_eq!(of("occasions", "host", "42"), 38);
        assert_eq!(of("paywall", "host", "1"), 94);
        assert_eq!(of("", "", ""), 59);
    }
}
