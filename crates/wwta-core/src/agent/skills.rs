//! Design guides for generated docs, written like skills: a name and a one-line description
//! the model sees up front, and a body it loads with the `load_skill` tool when it needs it.
//! `base` is the exception: it holds the rules every doc follows and is always in the prompt.

pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
}

const SOURCES: &[&str] = &[
    include_str!("skills/base.md"),
    include_str!("skills/mindmap.md"),
    include_str!("skills/timeline.md"),
    include_str!("skills/flashcards.md"),
    include_str!("skills/quiz.md"),
    include_str!("skills/minutes.md"),
    include_str!("skills/glossary.md"),
    include_str!("skills/report.md"),
];

/// Split `---\nname: x\ndescription: y\n---\nbody` into a skill.
fn parse(src: &str) -> Skill {
    // a Windows checkout may have CRLF line endings
    let src = src.replace("\r\n", "\n");
    let rest = src
        .strip_prefix("---\n")
        .expect("skill starts with front matter");
    let (front, body) = rest
        .split_once("\n---\n")
        .expect("front matter ends with ---");
    let field = |key: &str| {
        front
            .lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
            .map(|v| v.trim().to_string())
            .unwrap_or_else(|| panic!("skill front matter needs `{key}`"))
    };
    Skill {
        name: field("name"),
        description: field("description"),
        body: body.trim().to_string(),
    }
}

fn all() -> &'static [Skill] {
    static SKILLS: std::sync::LazyLock<Vec<Skill>> =
        std::sync::LazyLock::new(|| SOURCES.iter().map(|s| parse(s)).collect());
    &SKILLS
}

/// The always-included rules for docs.
pub fn base() -> &'static str {
    &all()
        .iter()
        .find(|s| s.name == "base")
        .expect("base skill")
        .body
}

/// Skills the model can load (everything except `base`).
pub fn loadable() -> impl Iterator<Item = &'static Skill> {
    all().iter().filter(|s| s.name != "base")
}

pub fn get(name: &str) -> Option<&'static Skill> {
    let name = name
        .trim()
        .trim_matches(['"', '\''])
        .to_lowercase()
        .replace([' ', '_'], "-");
    // accept "mind-map" for "mindmap", "meeting-minutes" for "minutes"
    let squashed = name.replace('-', "");
    loadable()
        .find(|s| s.name == name || s.name == squashed)
        .or_else(|| loadable().find(|s| squashed.contains(s.name.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_skill() {
        assert!(base().contains("WWTA.seek"));
        let names: Vec<_> = loadable().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "mindmap",
                "timeline",
                "flashcards",
                "quiz",
                "minutes",
                "glossary",
                "report"
            ]
        );
        assert!(loadable().all(|s| !s.description.is_empty() && s.body.starts_with("# ")));
    }

    #[test]
    fn finds_skills_leniently() {
        assert_eq!(get("Mind map").unwrap().name, "mindmap");
        assert_eq!(get("meeting_minutes").unwrap().name, "minutes");
        assert_eq!(get("\"quiz\"").unwrap().name, "quiz");
        assert!(get("poem").is_none());
        assert!(get("base").is_none());
    }
}
