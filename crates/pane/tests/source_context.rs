use pane::project::source_context::{ContextRole, pack};
use pane::sandbox::profile::Profile;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pane-source-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }
    fn profile(&self) -> Profile {
        Profile::compile(&self.root, None)
    }
    fn put(&self, path: &str, text: &str) -> PathBuf {
        let p = self.root.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, text).unwrap();
        p
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn python_pack_has_complete_definition_helpers_and_ranked_tests() {
    let f = Fixture::new("python");
    let padding = "# padding padding padding padding\n".repeat(600);
    let source = format!(
        "import os\nfrom lib import Thing\n\ndef helper(x):\n    return x + 1\n\n@decorator\ndef target(value):\n    if value:\n        return helper(value)\n    return 0\n\ndef after():\n    return 'outside'\n{padding}"
    );
    let target = f.put("src/mod.py", &source);
    f.put("src/use.py", "from mod import target\nresult = target(2)\n");
    f.put(
        "tests/test_mod.py",
        "def test_target():\n    assert target(1) == 2\n",
    );
    let got = pack(&f.profile(), &target, Some("target")).unwrap();
    assert!(got.complete);
    assert_eq!(got.symbol.as_deref(), Some("target"));
    assert!(got.target.text.starts_with("@decorator\ndef target"));
    assert!(got.target.text.contains("return 0"));
    assert!(!got.target.text.contains("def after"));
    assert_eq!(
        got.sha256,
        format!("{:x}", Sha256::digest(source.as_bytes()))
    );
    assert!(
        got.supporting
            .iter()
            .any(|e| e.role == ContextRole::NearbyDefinition && e.text.contains("def helper"))
    );
    let refs: Vec<_> = got
        .supporting
        .iter()
        .filter(|e| matches!(e.role, ContextRole::Test | ContextRole::Caller))
        .collect();
    assert_eq!(refs[0].role, ContextRole::Test);
    assert!(got.render().contains("omission: reference index visited"));
}

#[test]
fn rust_braces_in_strings_and_comments_do_not_clip_definition() {
    let f = Fixture::new("rust");
    let padding = "// padding padding padding padding\n".repeat(600);
    let source = format!(
        "use crate::Thing;\n\nfn helper() {{}}\n\n#[inline]\npub fn target<'a>() {{\n    let fake = \"}}\"; // }}\n    let raw = r###\"}}}}\"###;\n    /* }} and nested /* {{ */ still comment */\n    if fake.len() > 1 {{ helper(); }}\n}}\n\nfn after() {{ panic!() }}\n{padding}"
    );
    let p = f.put("src/lib.rs", &source);
    let got = pack(&f.profile(), &p, Some("target")).unwrap();
    assert!(got.target.text.contains("if fake.len()"));
    assert!(got.target.text.ends_with('}'));
    assert!(!got.target.text.contains("fn after"));
    assert!(
        got.supporting
            .iter()
            .any(|e| e.role == ContextRole::NearbyDefinition && e.text.contains("fn helper"))
    );
}

#[test]
fn typescript_parser_packs_decorated_target_and_ranks_tests() {
    let f = Fixture::new("typescript");
    let padding = "// padding padding padding padding\n".repeat(600);
    let source = format!(
        "import {{ helper }} from './helper';\n\nexport function nearby() {{ return helper(); }}\n\n/** Service documentation. */\n@sealed\nexport class Target {{\n    run(value: string) {{\n        const template = `literal }}}} ${{value}}`;\n        const pattern = /[}}]/u;\n        return {{ template, pattern }};\n    }}\n}}\n\nexport function after() {{ return 'outside'; }}\n{padding}"
    );
    let target = f.put("src/target.ts", &source);
    f.put(
        "src/caller.ts",
        "import { Target } from './target';\nnew Target().run('x');\n",
    );
    f.put(
        "tests/target.test.ts",
        "import { Target } from '../src/target';\ntest('Target', () => new Target());\n",
    );

    let got = pack(&f.profile(), &target, Some("Target")).unwrap();
    assert!(got.complete);
    assert_eq!(got.language, "typescript");
    assert!(
        got.target
            .text
            .starts_with("/** Service documentation. */\n@sealed")
    );
    assert!(got.target.text.contains("`literal }} ${value}`"));
    assert!(got.target.text.contains("/[}]/u"));
    assert!(!got.target.text.contains("function after"));
    assert!(got.supporting.iter().any(|excerpt| {
        excerpt.role == ContextRole::NearbyDefinition && excerpt.text.contains("function nearby")
    }));
    let references: Vec<_> = got
        .supporting
        .iter()
        .filter(|excerpt| matches!(excerpt.role, ContextRole::Test | ContextRole::Caller))
        .collect();
    assert_eq!(references[0].role, ContextRole::Test);
}

#[test]
fn malformed_typescript_uses_truthful_bounded_fallback() {
    let f = Fixture::new("typescript-malformed");
    let padding = "// padding padding padding padding\n".repeat(600);
    let source = format!(
        "export function target() {{\n    const broken = `unterminated;\n    return 1;\n}}\n{padding}"
    );
    let target = f.put("src/target.ts", &source);
    let got = pack(&f.profile(), &target, Some("target")).unwrap();

    assert!(!got.complete);
    assert!(got.target.text.contains("function target"));
    assert!(
        got.omissions
            .iter()
            .any(|note| { note.contains("complete definition boundary unavailable") })
    );
}

#[test]
fn javascript_parser_handles_template_and_regex_braces() {
    let f = Fixture::new("javascript");
    let padding = "// padding padding padding padding\n".repeat(600);
    let source = format!(
        "function target(value) {{\n    const template = `literal }}}} ${{value}}`;\n    const pattern = /[}}]/u;\n    return pattern.test(template);\n}}\n\nfunction after() {{ return false; }}\n{padding}"
    );
    let target = f.put("src/target.js", &source);
    let got = pack(&f.profile(), &target, Some("target")).unwrap();

    assert!(got.complete);
    assert_eq!(got.language, "javascript");
    assert!(got.target.text.contains("`literal }} ${value}`"));
    assert!(got.target.text.contains("/[}]/u"));
    assert!(!got.target.text.contains("function after"));
}

#[test]
fn go_scanner_packs_documented_target_and_ranks_tests() {
    let f = Fixture::new("go");
    let padding = "// padding padding padding padding\n".repeat(600);
    let source = format!(
        "package quota\n\nimport \"strings\"\n\ntype Service struct {{}}\n\nfunc helper() string {{ return \"ok\" }}\n\n/* func (Service) target() string {{ return \"comment\" }} */\n\n// target reserves capacity.\nfunc (Service) target(value string) string {{\n\traw := `literal }}}}`\n\tquoted := \"}}\"\n\t/* a block comment with }} */\n\tif strings.TrimSpace(value) != \"\" {{ return raw + quoted + helper() }}\n\treturn \"empty\"\n}}\n\nfunc after() string {{ return \"outside\" }}\n{padding}"
    );
    let target = f.put("quota.go", &source);
    f.put(
        "caller.go",
        "package quota\n\nvar result = Service{}.target(\"x\")\n",
    );
    f.put(
        "quota_test.go",
        "package quota\n\nfunc TestTarget(t *testing.T) { _ = Service{}.target(\"x\") }\n",
    );

    let got = pack(&f.profile(), &target, Some("target")).unwrap();
    assert!(got.complete);
    assert_eq!(got.language, "go");
    assert!(got.target.text.starts_with("// target reserves capacity."));
    assert!(got.target.text.contains("`literal }}`"));
    assert!(got.target.text.contains("return \"empty\""));
    assert!(!got.target.text.contains("func after"));
    assert!(got.supporting.iter().any(|excerpt| {
        excerpt.role == ContextRole::NearbyDefinition && excerpt.text.contains("func helper")
    }));
    let references: Vec<_> = got
        .supporting
        .iter()
        .filter(|excerpt| matches!(excerpt.role, ContextRole::Test | ContextRole::Caller))
        .collect();
    assert_eq!(references[0].role, ContextRole::Test);
}

#[test]
fn java_scanner_packs_annotated_target_and_ranks_tests() {
    let f = Fixture::new("java");
    let padding = "// padding padding padding padding\n".repeat(600);
    let source = format!(
        "package quota;\n\nimport java.util.regex.Pattern;\n\nclass Helpers {{ static int helper() {{ return 1; }} }}\n\npublic class Target {{\n    /* public int reserve(String value) {{ return 0; }} */\n\n    /** Reserve capacity. */\n    @Override\n    public int reserve(String value) {{\n        String brace = \"}}\";\n        String block = \"\"\"\n            literal }}}}\n            \"\"\";\n        /* }} is not the method boundary */\n        if (Pattern.matches(\"[}}]\", value)) {{ return Helpers.helper(); }}\n        return block.length() + brace.length();\n    }}\n}}\n\nclass After {{}}\n{padding}"
    );
    let target = f.put("src/Target.java", &source);
    f.put(
        "src/Caller.java",
        "class Caller { int call() { return new Target().reserve(\"x\"); } }\n",
    );
    f.put(
        "src/test/TargetTest.java",
        "class TargetTest { void targetWorks() { new Target().reserve(\"x\"); } }\n",
    );

    let got = pack(&f.profile(), &target, Some("reserve")).unwrap();
    assert!(got.complete);
    assert_eq!(got.language, "java");
    assert!(
        got.target
            .text
            .starts_with("    /** Reserve capacity. */\n    @Override")
    );
    assert!(got.target.text.contains("literal }}"));
    assert!(got.target.text.contains("return block.length()"));
    assert!(!got.target.text.contains("class After"));
    assert!(got.supporting.iter().any(|excerpt| {
        excerpt.role == ContextRole::NearbyDefinition && excerpt.text.contains("class Helpers")
    }));
    let references: Vec<_> = got
        .supporting
        .iter()
        .filter(|excerpt| matches!(excerpt.role, ContextRole::Test | ContextRole::Caller))
        .collect();
    assert_eq!(references[0].role, ContextRole::Test);
}

#[test]
fn malformed_go_and_java_use_truthful_bounded_fallbacks() {
    let f = Fixture::new("brace-malformed");
    let padding = "// padding padding padding padding\n".repeat(600);
    for (path, symbol, source) in [
        (
            "target.go",
            "target",
            format!("package bad\nfunc target() {{\n    value := `unterminated\n}}\n{padding}"),
        ),
        (
            "Target.java",
            "Target",
            format!(
                "class Target {{\n    String value = \"\"\"\n        unterminated\n}}\n{padding}"
            ),
        ),
        (
            "missing.go",
            "target",
            format!("package bad\nfunc target()\nfunc after() {{}}\n{padding}"),
        ),
        (
            "Missing.java",
            "target",
            format!(
                "abstract class Missing {{\n    abstract int target();\n    int after() {{ return 1; }}\n}}\n{padding}"
            ),
        ),
    ] {
        let target = f.put(path, &source);
        let got = pack(&f.profile(), &target, Some(symbol)).unwrap();
        assert!(!got.complete, "{path}");
        assert!(
            got.omissions
                .iter()
                .any(|note| { note.contains("complete definition boundary unavailable") })
        );
    }
}

#[test]
fn added_language_definition_caps_refuse_oversized_targets() {
    let f = Fixture::new("language-definition-caps");
    let cases = [
        (
            "target.ts",
            format!(
                "export function target() {{\n{}return 1;\n}}\n",
                "    // target padding padding padding\n".repeat(900)
            ),
            "target",
        ),
        (
            "target.go",
            format!(
                "package cap\nfunc target() int {{\n{}return 1\n}}\n",
                "    // target padding padding padding\n".repeat(900)
            ),
            "target",
        ),
        (
            "Target.java",
            format!(
                "class Target {{\n{}int value() {{ return 1; }}\n}}\n",
                "    // target padding padding padding\n".repeat(900)
            ),
            "Target",
        ),
    ];
    for (path, source, symbol) in cases {
        let target = f.put(path, &source);
        let error = pack(&f.profile(), &target, Some(symbol)).unwrap_err();
        assert!(
            error.0.contains("definition exceeds the 24000 byte cap"),
            "{path}: {error:?}"
        );
    }
}

#[test]
fn added_languages_infer_one_incomplete_definition() {
    let f = Fixture::new("language-inference");
    let padding = "// padding padding padding padding\n".repeat(600);
    for (path, source) in [
        (
            "target.ts",
            format!(
                "export function target() {{\n    // TODO: implement\n    return 1;\n}}\n{padding}"
            ),
        ),
        (
            "target.go",
            format!(
                "package infer\nfunc target() int {{\n    // TODO: implement\n    return 1\n}}\n{padding}"
            ),
        ),
        (
            "Target.java",
            format!(
                "class Target {{\n    int target() {{\n        // TODO: implement\n        return 1;\n    }}\n}}\n{padding}"
            ),
        ),
    ] {
        let target = f.put(path, &source);
        let got = pack(&f.profile(), &target, None).unwrap();
        assert!(got.complete, "{path}");
        assert_eq!(got.symbol.as_deref(), Some("target"), "{path}");
        assert!(got.omissions.iter().any(|note| note.contains("inferred")));
    }
}

#[test]
fn small_generic_file_is_returned_whole_with_typed_range() {
    let f = Fixture::new("small");
    let p = f.put("notes.xyz", "alpha\nβeta\nomega\n");
    let got = pack(&f.profile(), &p, None).unwrap();
    assert_eq!(got.target.role, ContextRole::CompleteFile);
    assert_eq!(got.target.range.start, 1);
    assert_eq!(got.target.range.end, 3);
    assert_eq!(got.target.text, "alpha\nβeta\nomega");
    assert!(got.complete);
}

#[test]
fn large_file_without_symbol_is_bounded_incomplete_and_requests_a_symbol() {
    let f = Fixture::new("large-no-symbol");
    let p = f.put("src/large.py", &"value = 1\n".repeat(5_000));
    let got = pack(&f.profile(), &p, None).unwrap();
    assert!(!got.complete);
    assert!(got.target.text.len() <= 24_000);
    assert!(
        got.omissions
            .iter()
            .any(|note| note.contains("supply `symbol`"))
    );
}

#[test]
fn unique_incomplete_definition_is_inferred_without_a_symbol() {
    let f = Fixture::new("infer-stub");
    let padding = "# padding padding padding padding\n".repeat(700);
    let source = format!(
        "def complete():\n    return 1\n\ndef reserve(value):\n    raise NotImplementedError('stub')\n\ndef after():\n    return 2\n{padding}"
    );
    let p = f.put("src/quota.py", &source);
    let got = pack(&f.profile(), &p, None).unwrap();
    assert!(got.complete);
    assert_eq!(got.symbol.as_deref(), Some("reserve"));
    assert!(got.target.text.starts_with("def reserve"));
    assert!(!got.target.text.contains("def after"));
    assert!(got.omissions.iter().any(|note| note.contains("inferred")));
}

#[test]
fn denial_and_oversize_are_errors_not_partial_context() {
    let f = Fixture::new("bounds");
    let secret = f.put("secret.py", "def target():\n    pass\n");
    let deny = format!(
        r#"{{"permissions":{{"deny":["Read({})"]}}}}"#,
        secret.display()
    );
    assert!(
        pack(
            &Profile::compile(&f.root, Some(&deny)),
            &secret,
            Some("target")
        )
        .unwrap_err()
        .0
        .contains("refused")
    );
    let huge = f.put("huge.py", &"x".repeat(1_048_577));
    assert!(
        pack(&f.profile(), &huge, None)
            .unwrap_err()
            .0
            .contains("source cap")
    );
}

#[test]
fn traversal_cap_is_reported_in_structured_omissions() {
    let f = Fixture::new("visit-cap");
    let target = f.put("a.py", "def target():\n    return 1\n");
    for i in 0..2_050 {
        f.put(&format!("noise/{i:04}.txt"), "irrelevant");
    }
    let got = pack(&f.profile(), &target, Some("target")).unwrap();
    assert!(
        got.omissions
            .iter()
            .any(|note| note.contains("2048-entry cap"))
    );
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_refused_by_profile() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new("escape");
    let outside = std::env::temp_dir().join(format!("pane-outside-{}", std::process::id()));
    fs::write(&outside, "secret").unwrap();
    let link = f.root.join("link.py");
    symlink(&outside, &link).unwrap();
    assert!(pack(&f.profile(), &link, None).is_err());
    let _ = fs::remove_file(outside);
}
