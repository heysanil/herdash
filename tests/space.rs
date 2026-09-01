//! Space-rename bookkeeping and crash recovery.
//!
//! herdash renames its herdr space so it is identifiable in the sidebar with
//! no user configuration. `workspace.rename` has no reset-to-derived, so the
//! previous name must be remembered — and a herdash that is killed rather
//! than closed must not strand a renamed space forever.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use herdash::space::{Claim, Claims, restore_target, state_path};

static N: AtomicU32 = AtomicU32::new(0);

fn tmp() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("herdash-space-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d.join("spaces.json")
}

#[test]
fn a_claim_round_trips_through_the_state_file() {
    let path = tmp();
    let mut claims = Claims::default();
    claims.claim("w3S", "herdash", "herdr-manager");
    claims.save(&path).unwrap();

    let loaded = Claims::load(&path);
    assert_eq!(
        loaded, claims,
        "a crashed run must find exactly what it wrote"
    );
    assert_eq!(
        loaded.spaces["w3S"],
        Claim {
            applied: "herdash".into(),
            original: "herdr-manager".into()
        }
    );
}

#[test]
fn a_missing_or_corrupt_state_file_is_not_fatal() {
    assert_eq!(
        Claims::load(&PathBuf::from("/nope/missing.json")),
        Claims::default()
    );
    let path = tmp();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ not json").unwrap();
    assert_eq!(Claims::load(&path), Claims::default());
}

#[test]
fn releasing_removes_the_claim() {
    let mut claims = Claims::default();
    claims.claim("w1", "herdash", "old");
    assert!(claims.release("w1").is_some());
    assert!(
        claims.release("w1").is_none(),
        "a claim is released exactly once"
    );
}

#[test]
fn the_original_name_is_restored_when_ours_is_still_applied() {
    let claim = Claim {
        applied: "herdash".into(),
        original: "herdr-manager".into(),
    };
    assert_eq!(
        restore_target(&claim, "herdash"),
        Some("herdr-manager".to_string())
    );
}

/// If the user renamed the space themselves after herdash claimed it, their
/// choice must survive — restoring would silently undo their rename.
#[test]
fn a_user_rename_after_ours_is_never_clobbered() {
    let claim = Claim {
        applied: "herdash".into(),
        original: "herdr-manager".into(),
    };
    assert_eq!(restore_target(&claim, "my-own-name"), None);
    assert_eq!(restore_target(&claim, ""), None);
}

#[test]
fn several_spaces_are_tracked_independently() {
    let path = tmp();
    let mut claims = Claims::default();
    claims.claim("w1", "herdash", "alpha");
    claims.claim("w2", "herdash", "beta");
    claims.save(&path).unwrap();

    let mut loaded = Claims::load(&path);
    assert_eq!(loaded.release("w1").unwrap().original, "alpha");
    assert_eq!(loaded.release("w2").unwrap().original, "beta");
}

#[test]
fn state_lives_under_the_xdg_state_directory() {
    let p = state_path(&PathBuf::from("/home/u"));
    assert!(p.ends_with("herdash/spaces.json"), "got {p:?}");
    assert!(p.to_string_lossy().contains("state") || std::env::var_os("XDG_STATE_HOME").is_some());
}
