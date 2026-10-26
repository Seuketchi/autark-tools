//! What the roster shows, worked out from what `autark-agentd` sends: the team, a conversation's messages, previews and
//! times. No GTK here, so it can be tested.

use serde::Deserialize;

/// One agent in the roster.
#[derive(Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Member {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub colour: u8,
    #[serde(default)]
    pub job: String,
    #[serde(default)]
    pub last: String,
    #[serde(default)]
    pub time: i64,
    #[serde(default)]
    pub writing: bool,
}

pub fn team(json: &str) -> Vec<Member> {
    serde_json::from_str(json).unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Who {
    You,
    Agent,
}

/// One bubble in a conversation.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub who: Who,
    pub text: String,
    pub time: i64,
    /// The answer streaming into it, while it is being written.
    pub writing: Option<u32>,
    pub error: String,
}

/// A conversation's messages from its entries: what you said and what the agent answered. Other entries (Kith's
/// action cards, proposed facts) belong to the launcher's panel and aren't shown here. An answer with nothing in
/// it, no error and no writing under way is left out.
pub fn messages(json: &str) -> Vec<Message> {
    let entries: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    entries
        .iter()
        .filter_map(|entry| {
            let time = entry["time"].as_i64().unwrap_or(0);
            match entry["kind"].as_str()? {
                "you" => Some(Message { who: Who::You, text: entry["text"].as_str()?.to_owned(), time, writing: None, error: String::new() }),
                "answer" => {
                    let text = entry["text"].as_str().unwrap_or_default().to_owned();
                    let error = entry["error"].as_str().unwrap_or_default().to_owned();
                    let writing = entry["writing"].as_bool().unwrap_or(false).then(|| entry["answer"].as_u64().unwrap_or(0) as u32);
                    (!text.is_empty() || !error.is_empty() || writing.is_some()).then_some(Message { who: Who::Agent, text, time, writing, error })
                }
                _ => None,
            }
        })
        .collect()
}

/// Whether a time stamp goes between two messages: the first one, or after half an hour of quiet.
pub fn stamp_between(before: Option<i64>, next: i64) -> bool {
    before.is_none_or(|before| next - before >= 30 * 60)
}

/// A line of what was said last, for the roster: its first line, in one run of single spaces.
pub fn preview(text: &str) -> String {
    text.lines().map(str::trim).find(|line| !line.is_empty()).map(|line| line.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default()
}

/// Whether a member matches what is typed in the search: any word of it in its name, job or last words.
pub fn matches(member: &Member, query: &str) -> bool {
    let haystack = format!("{} {} {}", member.name, member.job, member.last).to_lowercase();
    query.to_lowercase().split_whitespace().all(|word| haystack.contains(word))
}

/// The avatar colours, as `autark-ai` numbers them.
pub const COLOURS: [&str; 8] = ["#48B5A0", "#F2A33A", "#6C63E8", "#9B6FE0", "#3D8BF2", "#E8743B", "#E5527D", "#5DB85C"];

/// The facts of an agent's memory, as the backend sends them.
pub fn facts(json: &str) -> Vec<String> {
    let facts: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    facts.iter().filter_map(|fact| fact["text"].as_str().map(str::to_owned)).collect()
}

pub fn colour(index: u8) -> &'static str {
    COLOURS[usize::from(index) % COLOURS.len()]
}

/// Adds a streamed piece at its offset, ignoring one already seen, as the launcher's panel does.
pub fn merge(text: &mut String, offset: u32, piece: &str) {
    let offset = offset as usize;
    let have = text.len();
    if offset >= have {
        text.push_str(piece);
    } else if offset + piece.len() > have
        && let Some(rest) = piece.get(have - offset..)
    {
        text.push_str(rest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversation_shows_what_you_and_the_agent_said() {
        let json = r#"[
            {"id":1,"time":100,"kind":"you","text":"hi"},
            {"id":2,"time":101,"kind":"actions","actions":[],"results":[]},
            {"id":3,"time":102,"kind":"answer","answer":4,"text":"Hello.","writing":false},
            {"id":4,"time":103,"kind":"memory","text":"x","state":"proposed"},
            {"id":5,"time":104,"kind":"answer","answer":5,"text":"","writing":false},
            {"id":6,"time":105,"kind":"answer","answer":6,"text":"","writing":true}
        ]"#;
        let shown = messages(json);
        assert_eq!(shown.len(), 3);
        assert_eq!((shown[0].who.clone(), shown[0].text.as_str()), (Who::You, "hi"));
        assert_eq!(shown[1].text, "Hello.");
        assert_eq!(shown[2].writing, Some(6));
    }

    #[test]
    fn an_answer_that_failed_is_kept_to_say_why() {
        let shown = messages(r#"[{"id":1,"time":1,"kind":"answer","answer":0,"text":"","writing":false,"error":"AI is off"}]"#);
        assert_eq!(shown[0].error, "AI is off");
    }

    #[test]
    fn nonsense_is_an_empty_team_and_conversation() {
        assert!(team("not json").is_empty());
        assert!(messages("{}").is_empty());
    }

    #[test]
    fn a_stamp_goes_first_and_after_half_an_hour() {
        assert!(stamp_between(None, 5));
        assert!(!stamp_between(Some(1000), 1000 + 29 * 60));
        assert!(stamp_between(Some(1000), 1000 + 30 * 60));
    }

    #[test]
    fn a_preview_is_the_first_line_in_single_spaces() {
        assert_eq!(preview("\n  booked   the venue\nand sent it"), "booked the venue");
        assert_eq!(preview(""), "");
    }

    #[test]
    fn search_needs_every_word_somewhere() {
        let member = Member { name: "Inbox Manager".into(), job: "Keeps the mail at zero".into(), ..Default::default() };
        assert!(matches(&member, "inbox zero"));
        assert!(!matches(&member, "inbox sales"));
        assert!(matches(&member, ""));
    }

    #[test]
    fn the_memory_shows_each_fact_in_order() {
        assert_eq!(facts(r#"[{"text":"A","added":1},{"text":"B","added":2}]"#), ["A", "B"]);
        assert!(facts("nonsense").is_empty());
    }

    #[test]
    fn colours_wrap_instead_of_failing() {
        assert_eq!(colour(8), colour(0));
    }

    #[test]
    fn a_piece_seen_twice_is_added_once() {
        let mut text = String::new();
        merge(&mut text, 0, "Hel");
        merge(&mut text, 0, "Hel");
        merge(&mut text, 3, "lo");
        merge(&mut text, 2, "llo");
        assert_eq!(text, "Hello");
    }
}
