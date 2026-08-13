//! Does the vocabulary list actually earn its place?
//!
//! Run explicitly:
//!   cargo test --release --test vocabulary -- --nocapture --ignored

const MANGLED: &[&str] = &[
    // What Parakeet actually produced when the speaker said "Flow".
    "so this is a test recording from the application film",
    "I pushed the change to the hyper land config",
    "open the file in nvm and check the pipe wire logs",
];

#[test]
#[ignore]
fn compare_with_and_without_vocabulary() {
    let path = flow::cleanup::model_path();
    if !path.is_file() {
        eprintln!("skipping: no cleanup model");
        return;
    }

    let bare = flow::cleanup::Cleaner::load(&path, vec![], None).expect("load");
    for raw in MANGLED {
        eprintln!("\nraw:        {raw:?}");
        eprintln!("no vocab:   {:?}", bare.clean_within(raw, std::time::Duration::from_secs(120)).expect("clean"));
    }
    drop(bare);

    let terms = vec![
        "Flow".to_string(),
        "Hyprland".to_string(),
        "Neovim".to_string(),
        "PipeWire".to_string(),
    ];
    let informed = flow::cleanup::Cleaner::load(&path, terms, None).expect("load");
    for raw in MANGLED {
        eprintln!("\nraw:        {raw:?}");
        eprintln!("with vocab: {:?}", informed.clean_within(raw, std::time::Duration::from_secs(120)).expect("clean"));
    }
}
