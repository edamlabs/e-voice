use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::talk::question::{Asked, EMPTY, LONG, OUTSIDE, SPOKEN_LONG, ToolQuestion, UNREAD};
use serde_json::{Value, json};

use Lang::{En, Es};

fn cmd(command: &str) -> Value {
    json!({"command": command})
}

async fn of(tool: &str, input: &Value, lang: Lang) -> Asked {
    ToolQuestion::of(tool, input, lang, Path::new("/nonexistent-project")).await
}

fn ask(text: &str) -> Asked {
    Asked::Ask(text.to_owned())
}

fn refuse(reason: &str) -> Asked {
    Asked::Refuse(reason.to_owned())
}

#[tokio::test]
async fn test_a_command_is_read_word_by_word_with_every_symbol_named_in_es_and_en() {
    let cases: [(&str, Lang, &str); 8] = [
        ("ls -la | head", Es, "¿Ejecuto: ls, guion la, barra vertical, head?"),
        ("ls -la | head", En, "Shall I run: ls, dash la, pipe, head?"),
        (
            "git commit -m Fix",
            Es,
            "¿Ejecuto: git, commit, guion m, mayúscula fix?",
        ),
        (
            "git commit -m Fix",
            En,
            "Shall I run: git, commit, dash m, capital fix?",
        ),
        ("README.md", Es, "¿Ejecuto: mayúsculas readme punto md?"),
        ("README.md", En, "Shall I run: capitals readme dot md?"),
        ("echo 42", Es, "¿Ejecuto: echo, 42?"),
        ("a", En, "Shall I run: a?"),
    ];
    for (command, lang, question) in cases {
        assert_eq!(
            of("Bash", &cmd(command), lang).await,
            ask(question),
            "{command:?} {lang:?}"
        );
    }
}

#[tokio::test]
async fn test_a_commands_own_paths_are_not_checked_it_is_asked_whole_not_refused_as_outside() {
    let asked = ToolQuestion::of("Bash", &cmd("cat /etc/passwd"), Es, Path::new("/nonexistent-project")).await;
    assert_eq!(asked, ask("¿Ejecuto: cat, barra etc barra passwd?"));
}

#[tokio::test]
async fn test_the_other_command_tools_ask_the_same_question() {
    for tool in ["Bash", "PowerShell", "Monitor"] {
        assert_eq!(of(tool, &cmd("ls"), Es).await, ask("¿Ejecuto: ls?"), "{tool}");
    }
}

#[tokio::test]
async fn test_a_command_that_is_too_long_or_on_several_lines_is_refused_unasked() {
    let exact = "a".repeat(160);
    assert_eq!(of("Bash", &cmd(&exact), Es).await, ask(&format!("¿Ejecuto: {exact}?")));
    for refused in ["a".repeat(161), "ls\nrm".to_owned(), "ls\r".to_owned(), "\n".to_owned()] {
        assert_eq!(of("Bash", &cmd(&refused), Es).await, refuse(LONG), "{refused:?}");
    }
    assert_eq!(
        of("Bash", &cmd(&"ñ".repeat(161)), En).await,
        refuse(LONG),
        "length counts characters, not bytes"
    );
}

#[tokio::test]
async fn test_a_command_that_cannot_be_read_or_rebuilt_is_refused_unasked() {
    for text in [
        "echo …",
        "echo 🎉",
        "tab\there",
        "double  space",
        " leading",
        "trailing ",
        "",
        "echo punto",
        "echo mayúscula",
        "echo barra vertical",
        "ẞ",
        "echo \u{7f}",
        "echo \0",
        "echo \u{200b}",
    ] {
        assert_eq!(of("Bash", &cmd(text), Es).await, refuse(UNREAD), "{text:?}");
    }
    assert_eq!(of("Bash", &cmd("echo dot"), En).await, refuse(UNREAD));
    assert_eq!(of("Bash", &cmd("echo capital"), En).await, refuse(UNREAD));
    assert_eq!(
        of("Bash", &cmd("echo punto"), En).await,
        ask("Shall I run: echo, punto?"),
        "a word is a marker or a name only in its own language"
    );
}

#[tokio::test]
async fn test_a_question_over_four_hundred_characters_is_refused_unasked() {
    assert_eq!(of("Bash", &cmd(&"|".repeat(160)), Es).await, refuse(SPOKEN_LONG));
    let fits = of("Bash", &cmd(&"|".repeat(60)), En).await;
    assert_eq!(
        fits,
        ask(&format!("Shall I run: {}?", vec!["pipe"; 60].join(" "))),
        "60 pipes read in 60 words and fit"
    );
}

#[tokio::test]
async fn test_missing_fields_are_refused_as_empty() {
    assert_eq!(of("Bash", &json!({}), Es).await, refuse(EMPTY));
    assert_eq!(of("Bash", &json!({"command": 7}), Es).await, refuse(EMPTY));
    assert_eq!(of("WebSearch", &json!({"q": "x"}), Es).await, refuse(EMPTY));
    assert_eq!(of("WebFetch", &json!({"url": "not a url"}), Es).await, refuse(EMPTY));
    assert_eq!(of("WebFetch", &json!({}), Es).await, refuse(EMPTY));
}

#[tokio::test]
async fn test_web_tools_are_read_by_host_or_query() {
    assert_eq!(
        of("WebFetch", &json!({"url": "https://docs.rs/x?y"}), Es).await,
        ask("¿Abro docs punto rs?")
    );
    assert_eq!(
        of("WebFetch", &json!({"url": "https://docs.rs/x?y"}), En).await,
        ask("Shall I open docs dot rs?")
    );
    assert_eq!(
        of("WebFetch", &json!({"url": "https://user:pw@evil.example:8443/p"}), En).await,
        ask("Shall I open evil dot example?")
    );
    assert_eq!(
        of("WebSearch", &json!({"query": "rust async"}), Es).await,
        ask("¿Busco en la web: rust, async?")
    );
    assert_eq!(
        of("WebSearch", &json!({"query": "rust async"}), En).await,
        ask("Shall I search the web for: rust, async?")
    );
}

#[tokio::test]
async fn test_any_other_tool_is_asked_by_its_name() {
    assert_eq!(
        of("mcp__github__create_issue", &json!({}), Es).await,
        ask("¿Uso mcp, github, create guion bajo issue?")
    );
    assert_eq!(
        of("mcp__github__create_issue", &json!({}), En).await,
        ask("Shall I use mcp, github, create underscore issue?")
    );
    assert_eq!(of("Task", &json!({"x": 1}), En).await, ask("Shall I use capital task?"));
}

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("question-{name}-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(root.join("project/src")).unwrap();
        fs::create_dir_all(root.join("outside")).unwrap();
        fs::write(root.join("outside/secret.txt"), "x").unwrap();
        fs::write(root.join("project/real.txt"), "x").unwrap();
        fs::write(root.join("project/src/a.rs"), "x").unwrap();
        Self { root }
    }

    fn cwd(&self) -> PathBuf {
        self.root.join("project")
    }

    async fn edit(&self, tool: &str, key: &str, path: &str, lang: Lang) -> Asked {
        ToolQuestion::of(tool, &json!({key: path}), lang, &self.cwd()).await
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

#[tokio::test]
async fn test_an_edit_inside_the_project_is_read_by_its_relative_path() {
    let project = Project::new("inside");
    let cwd = project.cwd();
    let absolute = cwd.join("src/a.rs").to_string_lossy().into_owned();
    for path in ["src/a.rs", absolute.as_str(), "./src/a.rs", "src/../src/a.rs"] {
        assert_eq!(
            project.edit("Edit", "file_path", path, Es).await,
            ask("¿Edito src barra a punto rs?"),
            "{path}"
        );
    }
    assert_eq!(
        project.edit("Edit", "file_path", "src/a.rs", En).await,
        ask("Shall I edit src slash a dot rs?")
    );
    assert_eq!(
        project.edit("Write", "file_path", "src/a.rs", Es).await,
        ask("¿Escribo src barra a punto rs?")
    );
    assert_eq!(
        project.edit("Write", "file_path", "src/a.rs", En).await,
        ask("Shall I write src slash a dot rs?")
    );
    assert_eq!(
        project.edit("NotebookEdit", "notebook_path", "n.ipynb", Es).await,
        ask("¿Edito n punto ipynb?")
    );
}

#[tokio::test]
async fn test_a_new_file_in_a_new_subdirectory_is_allowed_when_its_tail_is_plain() {
    let project = Project::new("new");
    assert_eq!(
        project.edit("Write", "file_path", "new/deep/file.rs", Es).await,
        ask("¿Escribo new barra deep barra file punto rs?")
    );
    assert_eq!(
        project.edit("Write", "file_path", "fresh.txt", En).await,
        ask("Shall I write fresh dot txt?")
    );
    assert_eq!(
        project.edit("Write", "file_path", "new/../x.rs", Es).await,
        refuse(OUTSIDE),
        "a `..` in the part that does not exist yet"
    );
}

#[tokio::test]
async fn test_a_path_outside_the_project_the_project_itself_or_no_path_is_refused_unasked() {
    let project = Project::new("outside");
    let cwd = project.cwd();
    let sibling = project.root.join("outside/secret.txt").to_string_lossy().into_owned();
    let root = cwd.to_string_lossy().into_owned();
    for path in [
        "/etc/passwd",
        "../x",
        "../outside/secret.txt",
        sibling.as_str(),
        root.as_str(),
        ".",
        "",
        "src/../..",
        "/",
    ] {
        assert_eq!(
            project.edit("Edit", "file_path", path, Es).await,
            refuse(OUTSIDE),
            "{path:?}"
        );
    }
    let none = ToolQuestion::of("Edit", &json!({}), Es, &cwd).await;
    assert_eq!(none, refuse(OUTSIDE));
    let wrong = ToolQuestion::of("Edit", &json!({"file_path": 7}), Es, &cwd).await;
    assert_eq!(wrong, refuse(OUTSIDE));
}

#[tokio::test]
async fn test_symlinks_are_followed_like_the_tool_follows_them() {
    let project = Project::new("links");
    let cwd = project.cwd();
    symlink(project.root.join("outside"), cwd.join("out-dir")).unwrap();
    symlink(project.root.join("outside/secret.txt"), cwd.join("out-file")).unwrap();
    symlink(cwd.join("real.txt"), cwd.join("in-file")).unwrap();
    symlink(cwd.join("src"), cwd.join("in-dir")).unwrap();
    assert_eq!(
        project.edit("Edit", "file_path", "out-dir/secret.txt", Es).await,
        refuse(OUTSIDE),
        "a symlinked directory pointing out"
    );
    assert_eq!(
        project.edit("Write", "file_path", "out-dir/new.txt", Es).await,
        refuse(OUTSIDE),
        "a new file under it"
    );
    assert_eq!(
        project.edit("Edit", "file_path", "out-file", Es).await,
        refuse(OUTSIDE),
        "a symlinked file pointing out"
    );
    assert_eq!(
        project.edit("Edit", "file_path", "in-file", Es).await,
        ask("¿Edito real punto txt?"),
        "a symlinked file pointing in is read as its target"
    );
    assert_eq!(
        project.edit("Edit", "file_path", "in-dir/a.rs", Es).await,
        ask("¿Edito src barra a punto rs?")
    );
    symlink(cwd.join("missing-target"), cwd.join("dangling")).unwrap();
    let dangling = project.edit("Write", "file_path", "dangling", Es).await;
    assert!(matches!(dangling, Asked::Ask(_) | Asked::Refuse(_)), "{dangling:?}");
}

#[tokio::test]
async fn test_a_project_given_through_a_symlink_is_resolved_too() {
    let project = Project::new("rootlink");
    let link = project.root.join("link-to-project");
    symlink(project.cwd(), &link).unwrap();
    let asked = ToolQuestion::of("Edit", &json!({"file_path": "src/a.rs"}), Es, &link).await;
    assert_eq!(asked, ask("¿Edito src barra a punto rs?"));
    let outside = ToolQuestion::of("Edit", &json!({"file_path": "../outside/secret.txt"}), Es, &link).await;
    assert_eq!(outside, refuse(OUTSIDE));
}

#[tokio::test]
async fn test_a_project_that_does_not_exist_refuses_every_file() {
    let asked = ToolQuestion::of(
        "Edit",
        &json!({"file_path": "a.rs"}),
        Es,
        Path::new("/nonexistent-project-dir"),
    )
    .await;
    assert_eq!(asked, refuse(OUTSIDE));
}

/// The frame words around what is read, as `of` puts them for a command.
fn unframe(question: &str, lang: Lang) -> Option<&str> {
    let before = match lang {
        Es => "¿Ejecuto: ",
        En => "Shall I run: ",
    };
    question.strip_prefix(before)?.strip_suffix('?')
}

#[tokio::test]
async fn test_rebuild_inverts_the_reading_of_the_examples() {
    for (spoken, lang, text) in [
        ("ls, guion la, barra vertical, head", Es, "ls -la | head"),
        ("git, commit, guion m, mayúscula fix", Es, "git commit -m Fix"),
        ("mayúsculas readme punto md", Es, "README.md"),
        ("ls, dash la, pipe, head", En, "ls -la | head"),
        ("capitals readme dot md", En, "README.md"),
        ("capital http, capitals server", En, "Http SERVER"),
    ] {
        assert_eq!(ToolQuestion::rebuild(spoken, lang).as_deref(), Some(text), "{spoken}");
    }
    assert_eq!(
        ToolQuestion::rebuild("mayúscula", Es),
        None,
        "a marker with nothing after it"
    );
    assert_eq!(ToolQuestion::rebuild("a, capitals", En), None);
}

const LOWER: &[&str] = &[
    "ls",
    "echo",
    "git",
    "ñandú",
    "café",
    "x",
    "über",
    "日本語",
    "commit",
    "a",
];
const TITLE: &[&str] = &["Fix", "Hello", "Ñandú", "Zed", "A1"];
const CAPS: &[&str] = &["README", "HTTP", "ID", "OK", "HTTPServer", "ABc"];
const DIGITS: &[&str] = &["0", "42", "2026", "007"];
const SYMBOLS: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";
const ODD: &[&str] = &["…", "🎉", "ẞ", "\t", "\u{7f}", "\0", "é", "ǅ", "Ⅷ", "²"];
const WORDS_LIKE_NAMES: &[&str] = &[
    "punto",
    "barra",
    "pipe",
    "dot",
    "capital",
    "capitals",
    "mayúscula",
    "mayúsculas",
    "guion",
    "bajo",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap_or(1)).unwrap_or_default()
    }

    fn pick<'a>(&mut self, bank: &[&'a str]) -> &'a str {
        bank[self.below(bank.len())]
    }
}

fn command(seed: u64) -> String {
    let mut rng = Rng(seed);
    let words = 1 + rng.below(6);
    (0..words)
        .map(|_| {
            let parts = 1 + rng.below(4);
            (0..parts)
                .map(|_| match rng.below(14) {
                    0..=3 => rng.pick(LOWER).to_owned(),
                    4 => rng.pick(TITLE).to_owned(),
                    5 => rng.pick(CAPS).to_owned(),
                    6 | 7 => rng.pick(DIGITS).to_owned(),
                    8..=11 => SYMBOLS
                        .chars()
                        .nth(rng.below(SYMBOLS.chars().count()))
                        .map(String::from)
                        .unwrap_or_default(),
                    12 => rng.pick(WORDS_LIKE_NAMES).to_owned(),
                    _ if rng.below(5) == 0 => rng.pick(ODD).to_owned(),
                    _ => rng.pick(LOWER).to_owned(),
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(if rng.below(40) == 0 { "  " } else { " " })
}

#[tokio::test]
async fn test_what_is_asked_is_faithful_the_exact_command_comes_back_from_the_question() {
    const RUNS: u64 = 6_000;
    let mut asked = 0usize;
    let mut refused = 0usize;
    let mut seen: HashMap<(String, bool), String> = HashMap::new();
    let mut failures = Vec::new();
    for seed in 0..RUNS {
        let text = command(seed);
        for lang in [Es, En] {
            match of("Bash", &cmd(&text), lang).await {
                Asked::Ask(question) => {
                    asked += 1;
                    let rebuilt = unframe(&question, lang).and_then(|spoken| ToolQuestion::rebuild(spoken, lang));
                    if rebuilt.as_deref() != Some(text.as_str()) {
                        failures.push(format!(
                            "seed {seed} {lang:?}: {text:?} asked {question:?} which rebuilds to {rebuilt:?}"
                        ));
                    }
                    if question.chars().count() > 400 || text.chars().count() > 160 || text.contains(['\n', '\r']) {
                        failures.push(format!("seed {seed}: asked something over a cap: {text:?}"));
                    }
                    let other = seen.insert((question.clone(), lang == Es), text.clone());
                    if other.as_ref().is_some_and(|other| *other != text) {
                        failures.push(format!(
                            "two commands, one question {question:?}: {other:?} and {text:?}"
                        ));
                    }
                }
                Asked::Refuse(reason) => {
                    refused += 1;
                    if ![LONG, UNREAD, SPOKEN_LONG].contains(&reason.as_str()) {
                        failures.push(format!("seed {seed}: an unknown reason {reason:?}"));
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures, first 12:\n{}",
        failures.len(),
        failures.iter().take(12).cloned().collect::<Vec<_>>().join("\n")
    );
    assert!(
        asked > 4_000 && refused > 500,
        "the generator must reach both outcomes: asked {asked}, refused {refused}"
    );
}

#[tokio::test]
async fn test_every_ascii_symbol_alone_is_asked_and_rebuilds() {
    for symbol in SYMBOLS.chars() {
        let text = symbol.to_string();
        for lang in [Es, En] {
            let Asked::Ask(question) = of("Bash", &cmd(&text), lang).await else {
                panic!("{text:?} refused")
            };
            let rebuilt = unframe(&question, lang).and_then(|spoken| ToolQuestion::rebuild(spoken, lang));
            assert_eq!(rebuilt.as_deref(), Some(text.as_str()), "{symbol} {lang:?}: {question}");
        }
    }
}
