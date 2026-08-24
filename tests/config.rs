//! Defaults must stand on their own, a partial file must only move the keys it
//! names, and a typo must be loud rather than silently ignored.

use flow::config::Config;
use flow::refine::Cleanup;

#[test]
fn defaults_need_no_file() {
    let defaults = Config::default();
    assert!(defaults.push_to_talk);
    assert_eq!(defaults.cleanup, Cleanup::Light);
    assert_eq!(defaults.duck, 50);
}

#[test]
fn missing_file_is_the_normal_state() {
    let absent = std::env::temp_dir().join("flow-no-such-config.toml");
    assert_eq!(
        Config::load_from(&absent).expect("absent"),
        Config::default()
    );
}

#[test]
fn file_overrides_every_key() {
    let parsed =
        Config::parse("push_to_talk = false\nduck = 20\ncleanup = medium\ndenoise = true\n")
            .expect("parse");
    assert_eq!(
        parsed,
        Config {
            push_to_talk: false,
            duck: 20,
            cleanup: Cleanup::Medium,
            chord: Default::default(),
            gpu: None,
            denoise: true,
            sound: true,
            record_debug: false,
        }
    );
}

#[test]
fn partial_file_keeps_the_other_defaults() {
    let parsed = Config::parse("duck = 0\n").expect("parse");
    assert_eq!(parsed.duck, 0);
    assert_eq!(parsed.push_to_talk, Config::default().push_to_talk);
    assert_eq!(parsed.cleanup, Config::default().cleanup);
}

/// `refine` was the only switch before levels existed. An existing config still
/// says it, and must keep meaning what it meant.
#[test]
fn the_old_refine_key_still_parses() {
    assert_eq!(
        Config::parse("refine = false\n").expect("parse").cleanup,
        Cleanup::None
    );
    assert_eq!(
        Config::parse("refine = true\n").expect("parse").cleanup,
        Cleanup::Light
    );
}

/// `hard` was a fourth level until it measured indistinguishable from Medium.
/// Anyone who selected it has it written to disk, and the daemon refusing to
/// start over a level that no longer exists would be a far worse regression
/// than silently giving them the closest one that does.
#[test]
fn the_removed_hard_level_still_starts_the_daemon() {
    assert_eq!(
        Config::parse("cleanup = hard\n").expect("hard").cleanup,
        Cleanup::Medium
    );
}

#[test]
fn an_unknown_cleanup_level_is_loud() {
    let err = Config::parse("cleanup = aggressive\n").expect_err("bad level");
    assert!(err.to_string().contains("cleanup"), "{err}");
}

#[test]
fn comments_blank_lines_and_spacing_are_ignored() {
    let parsed =
        Config::parse("# how flow behaves\n\n  push_to_talk=false  \nduck = 30 # halve music\n")
            .expect("parse");
    assert!(!parsed.push_to_talk);
    assert_eq!(parsed.duck, 30);
}

/// Absent means "pick the best GPU yourself", which is the normal state. An
/// explicit index only exists for machines where that choice is wrong.
#[test]
fn the_gpu_index_is_optional() {
    assert_eq!(Config::default().gpu, None);
    assert_eq!(Config::parse("gpu = 1\n").expect("parse").gpu, Some(1));
    assert_eq!(Config::parse("duck = 20\n").expect("parse").gpu, None);
    assert!(Config::parse("gpu = nvidia\n").is_err());
}

/// The binding is text in the config so a collision with an existing desktop
/// shortcut is fixable without rebuilding.
#[test]
fn the_hotkey_is_configurable() {
    assert_eq!(Config::default().chord.to_string(), "super+shift+d");
    assert_eq!(
        Config::parse("hotkey = ctrl+alt+space\n")
            .expect("parse")
            .chord
            .to_string(),
        "ctrl+alt+space"
    );
    assert_eq!(
        Config::parse("hotkey = rightctrl\n")
            .expect("parse")
            .chord
            .to_string(),
        "rightctrl",
        "the old bare-key default must stay expressible"
    );

    let err = Config::parse("hotkey = super+shft+d\n").expect_err("typo");
    assert!(err.to_string().contains("line 1"), "{err}");
}

#[test]
fn a_typo_names_itself() {
    let err = Config::parse("puhs_to_talk = false\n").expect_err("unknown key");
    let message = err.to_string();
    assert!(message.contains("puhs_to_talk"), "{message}");
    assert!(message.contains("line 1"), "{message}");
}

#[test]
fn a_bad_value_names_its_line() {
    let err = Config::parse("refine = true\nduck = loud\n").expect_err("bad number");
    assert!(err.to_string().contains("line 2"), "{err}");

    let err = Config::parse("refine = yes\n").expect_err("bad bool");
    assert!(err.to_string().contains("refine"), "{err}");
}

#[test]
fn duck_is_a_percentage() {
    assert!(Config::parse("duck = 100\n").is_ok());
    assert!(Config::parse("duck = 101\n").is_err());
}

/// The same value has to mean the same thing from either entry point. `--duck
/// 200` used to sail past the check the file enforces, and since ducking clamps
/// to 100 - which is "leave every stream where it is" - the typo silently turned
/// ducking off instead of turning it up.
#[test]
fn an_out_of_range_duck_flag_does_not_silently_disable_ducking() {
    let from_flag = Config::default().overridden_by(&["--duck".into(), "200".into()]);
    assert!(from_flag.duck <= 100, "flag kept {}", from_flag.duck);
    assert_eq!(
        from_flag.ducking(),
        Some(100),
        "a too-large value should clamp to full ducking, never to none"
    );
}

#[test]
fn a_line_without_a_value_is_an_error() {
    assert!(Config::parse("push_to_talk\n").is_err());
}

#[test]
fn flags_win_over_the_file() {
    let from_file = Config {
        push_to_talk: true,
        duck: 50,
        cleanup: Cleanup::Light,
        chord: Default::default(),
        gpu: None,
        denoise: false,
        sound: true,
        record_debug: false,
    };
    let flags = ["daemon", "--raw", "--duck", "20", "--denoise"]
        .map(String::from)
        .to_vec();

    let effective = from_file.clone().overridden_by(&flags);
    assert_eq!(effective.push_to_talk, from_file.push_to_talk);
    assert_eq!(effective.cleanup, Cleanup::None);
    assert!(effective.denoise);
    assert_eq!(effective.duck, 20);

    // `--no-ptt` skips the key watcher; it is not the tap-to-talk switch.
    assert_eq!(
        from_file
            .clone()
            .overridden_by(&["--no-ptt".into()])
            .push_to_talk,
        from_file.push_to_talk,
    );

    // `--cleanup` names a level outright, where `--raw` only ever means none.
    let levelled =
        from_file.overridden_by(["daemon", "--cleanup", "medium"].map(String::from).as_ref());
    assert_eq!(levelled.cleanup, Cleanup::Medium);
}

#[test]
fn absent_flags_leave_the_file_alone() {
    let from_file = Config {
        push_to_talk: false,
        duck: 20,
        cleanup: Cleanup::None,
        chord: Default::default(),
        gpu: None,
        denoise: false,
        sound: true,
        record_debug: false,
    };
    assert_eq!(
        from_file.clone().overridden_by(&[String::from("daemon")]),
        from_file
    );
}

#[test]
fn zero_duck_means_no_ducking() {
    assert_eq!(
        Config {
            duck: 0,
            ..Config::default()
        }
        .ducking(),
        None
    );
    assert_eq!(
        Config {
            duck: 50,
            ..Config::default()
        }
        .ducking(),
        Some(50)
    );

    let off_by_flag = Config::default().overridden_by(&["--duck".into(), "0".into()]);
    assert_eq!(off_by_flag.ducking(), None);
}

#[test]
fn the_shipped_template_parses_and_matches_the_defaults() {
    let template = include_str!("../packaging/config.template.toml");
    assert_eq!(
        Config::parse(template).expect("template"),
        Config::default()
    );
}

/// The chime is on out of the box, and the only thing the switch does is
/// silence it. Default-on because a dictation with no window focus is
/// otherwise unacknowledged until the text lands.
#[test]
fn the_dictation_sound_is_on_until_turned_off() {
    assert!(flow::config::Config::default().sound);
    assert!(flow::config::Config::parse("sound = true").unwrap().sound);
    assert!(!flow::config::Config::parse("sound = false").unwrap().sound);
    assert!(flow::config::Config::parse("sound = maybe").is_err());
}

#[test]
fn a_line_from_another_version_costs_only_that_line() {
    // The trap this closes: `~/.local/bin/flow` predating a key the console had
    // already written meant systemd started it, it read `sound = false`, and it
    // exited 1 - on a loop, with no dictation and no way back from the window.
    // A line it cannot understand now costs that line and nothing else.
    let (config, complaints) = Config::parse_forgiving(
        "push_to_talk = false\nfrom_a_newer_flow = 3\nduck = 20\ncleanup = walloped\n",
    );

    assert!(!config.push_to_talk, "a good line before the bad one");
    assert_eq!(config.duck, 20, "a good line after it");
    assert_eq!(
        config.cleanup,
        Cleanup::default(),
        "a value this version cannot read leaves the default"
    );

    assert_eq!(complaints.len(), 2, "{complaints:?}");
    assert!(
        complaints[0].contains("from_a_newer_flow"),
        "{complaints:?}"
    );
    assert!(complaints[0].contains("line 2"), "{complaints:?}");
    assert!(complaints[1].contains("line 4"), "{complaints:?}");
}

#[test]
fn a_whole_file_is_still_read_strictly() {
    // `parse` is what the live reload and the tests above lean on: it says no
    // rather than guessing, which is what keeps a typo loud.
    assert!(Config::parse("from_a_newer_flow = 3\n").is_err());
}
