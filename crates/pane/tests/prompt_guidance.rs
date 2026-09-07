use pane::prompt::{self, declarations};
use pane::tools::registry;

#[test]
fn preamble_states_the_observation_and_batching_decisions() {
    let text = prompt::PREAMBLE;
    for rule in [
        "unseen fields are not model-visible",
        "Reuse\nlive handles",
        "`context({path, symbol})` is the first source-reading tool",
        "`edit({path, old, replacement})`",
        "Batch deterministic work",
        "wait for its correlated result",
        "Never invent output or\ninfer success",
        "`glob` may return directories",
        "`exit_code`",
    ] {
        assert!(text.contains(rule), "missing guidance: {rule}");
    }
    assert!(text.contains("Answer conversational questions naturally"));
    assert!(text.contains("provider-native tool"));
}

#[test]
fn declarations_cover_tool_specific_decisions() {
    let mut names: Vec<_> = declarations::ENTRIES
        .iter()
        .map(|entry| entry.name)
        .collect();
    let mut registered = registry::names();
    names.sort_unstable();
    registered.sort_unstable();
    assert_eq!(names, registered);
    assert!(
        declarations::lookup("glob")
            .unwrap()
            .summary
            .contains("may include directories")
    );
    assert!(
        declarations::lookup("bash")
            .unwrap()
            .summary
            .contains("inspect `exit_code`")
    );
    assert!(
        declarations::lookup("context")
            .unwrap()
            .summary
            .contains("complete target")
    );
    assert!(
        declarations::lookup("edit")
            .unwrap()
            .summary
            .contains("Stale")
    );

    let runtime = declarations::RUNTIME
        .iter()
        .map(|binding| binding.declaration)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(runtime.contains("compact structured summary"));
    assert!(runtime.contains("Use background work only"));
    assert!(runtime.contains("Use it only when the question is separable"));
}
