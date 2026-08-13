//! Prompts — the canned starting points a user picks from a menu.
//!
//! Nearly free to build and the thing that makes this a feature rather than a
//! pile of tools: nobody opens a chat window and thinks "I should ask about the
//! play-weighted centroid of my genre distribution". They pick "how has my
//! taste moved this year" off a list.
//!
//! Each one is written to do two things at once — say what the user wants, and
//! tell the model which tools answer it and what not to claim. That second half
//! is why these are worth writing carefully: a prompt is the last place to
//! restate a rule before the model starts working.

use serde_json::{json, Value};

use super::scopes;
use crate::db::McpAuth;

struct PromptDef {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    scope: &'static str,
    args: &'static [(&'static str, &'static str, bool)],
    body: fn(&Value) -> String,
}

fn arg(args: &Value, key: &str, fallback: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn catalogue() -> Vec<PromptDef> {
    vec![
        PromptDef {
            name: "month_in_review",
            title: "Write my month in review",
            description: "A written-up chapter for a recent month: what was played, what was \
new, and what changed.",
            scope: scopes::LISTENING_READ,
            args: &[("offset", "How many months back (0 = the month just gone)", false)],
            body: |a| {
                let offset = arg(a, "offset", "0");
                format!(
                    "Write up my listening month as a short piece of prose I would want to \
read back in a year.\n\n\
Start with `listening_profile`, then `period_report` with unit=month and offset={offset}. \
Use `top_charts` and `artist` if you need to go deeper on something the report surfaced.\n\n\
Write about what actually happened — the artist that took over, the record I kept going \
back to, anything that arrived and stuck. Quote real numbers, and where a number has a \
coverage figure beside it, either say what it is a share of or leave it out.\n\n\
Do not grade the month and do not score my listening. If it was a quiet month, say it was \
a quiet month."
                )
            },
        },
        PromptDef {
            name: "what_should_i_put_on",
            title: "What should I put on?",
            description: "Something to listen to right now, from music I already have.",
            scope: scopes::LIBRARY_READ,
            args: &[("mood", "What I'm in the mood for, in my own words", false)],
            body: |a| {
                let mood = arg(a, "mood", "");
                let line = if mood.is_empty() {
                    String::from(
                        "I haven't said what I'm in the mood for — ask me, or read it \
off what I have been playing lately.",
                    )
                } else {
                    format!("What I'm after: {mood}")
                };
                format!(
                    "Help me decide what to put on right now. {line}\n\n\
Work from `rediscovery` first — it is built entirely from records I already own and is the \
right shape for this question. `loves`, `shelf` and `search_listens` fill in around it.\n\n\
EVERYTHING YOU SUGGEST MUST BE SOMETHING I ALREADY HAVE. This is a journal of my own \
collection, not a shop. If you want to mention a genre I own nothing in, say plainly that \
I do not have it.\n\n\
Give me three or four options, not a list of twenty, and one sentence each on why now."
                )
            },
        },
        PromptDef {
            name: "taste_trajectory",
            title: "How has my taste moved?",
            description: "Where my listening has travelled across the genre map, and what that \
looks like in records.",
            scope: scopes::LISTENING_READ,
            args: &[],
            body: |_| {
                String::from(
                    "Tell me how my taste has moved, and make it concrete.\n\n\
Use `genre_trajectory` for the shape of the movement and `genre_neighbours` for where it is \
heading. Then ground it: use `top_charts` over different ranges and `period_report` to name \
the actual artists and records behind each turn. A line on a map means nothing to me \
without the music that made it.\n\n\
Two things to be careful about. The trajectory is an ARTIST-MIX trajectory — genres attach \
to artists, so every track by an artist sits at the same point, and you must not describe \
the movement as finer than that. And in `genre_neighbours`, `next_door` is genres I already \
own music in while `unexplored` is genres I do not; any artists named in the second are not \
mine and you must say so.",
                )
            },
        },
        PromptDef {
            name: "build_me_a_playlist",
            title: "Build me a playlist",
            description: "Put together a playlist from my own library and save it to Tapedeck \
for me to look at.",
            scope: scopes::PLAYLISTS_WRITE,
            args: &[
                ("brief", "What the playlist is for, in my own words", true),
                ("length", "Roughly how many tracks", false),
            ],
            body: |a| {
                let brief = arg(a, "brief", "something I'd enjoy");
                let length = arg(a, "length", "20");
                format!(
                    "Build me a playlist: {brief}. About {length} tracks.\n\n\
Draw every track from my own listening — `search_listens`, `top_charts`, `loves` and \
`rediscovery` are where they should come from. Do not include anything I have never played; \
if you cannot fill the brief from what I have, make it shorter and tell me why.\n\n\
Sequence it deliberately rather than dumping it in rank order — this is a record you are \
putting together, and the running order is part of it.\n\n\
Then call `save_playlist` with a name, a one-sentence description saying what it is, and a \
`note` on each track saying why it is there. Tell me what you saved and let me review it — \
do not push it to a server unless I ask."
                )
            },
        },
        PromptDef {
            name: "how_am_i_listening",
            title: "How am I listening?",
            description: "The gear side: chains, hours, what is wearing out, and what I have \
never heard on what.",
            scope: scopes::LIBRARY_READ,
            args: &[],
            body: |_| {
                String::from(
                    "Tell me about how I have been listening, not just what.\n\n\
`signal_chains` and `equipment` are the heart of this — which paths I actually use, the \
hours through each, and whether anything is near the end of its service life. `shelf` for \
what I have been playing physically, `listening_sessions` for the sittings, and \
`rediscovery` can tell me what I have never heard on a given piece of gear.\n\n\
Audio format is NOT a score. A lossless share is a fact about files and often a fact about \
where the history came from — check its coverage in `listening_profile` before you mention \
it at all, and never present it as a verdict on how well I listen.",
                )
            },
        },
    ]
}

pub fn list(auth: &McpAuth) -> Vec<Value> {
    catalogue()
        .into_iter()
        .filter(|p| auth.has_scope(p.scope))
        .map(|p| {
            json!({
                "name": p.name,
                "title": p.title,
                "description": p.description,
                "arguments": p.args.iter().map(|(name, desc, required)| json!({
                    "name": name, "description": desc, "required": required,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

pub fn get(auth: &McpAuth, name: &str, args: &Value) -> Result<Value, String> {
    let def = catalogue().into_iter().find(|p| p.name == name).ok_or("Unknown prompt")?;
    if !auth.has_scope(def.scope) {
        return Err("This connection is not permitted to use that prompt".into());
    }
    Ok(json!({
        "description": def.description,
        "messages": [{
            "role": "user",
            "content": { "type": "text", "text": (def.body)(args) },
        }],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AuthUser;

    fn auth(scopes: &str) -> McpAuth {
        McpAuth {
            connection_id: 1,
            user: AuthUser { user_id: 1, username: "t".into(), is_admin: false },
            scopes: scopes.into(),
        }
    }

    #[test]
    fn a_prompt_is_not_offered_without_the_tools_it_drives() {
        // Offering "build me a playlist" to a connection that cannot save one
        // produces a conversation that ends in a failed tool call.
        let names: Vec<String> =
            list(&auth(scopes::LISTENING_READ)).iter().map(|p| p["name"].to_string()).collect();
        assert!(names.iter().any(|n| n.contains("month_in_review")));
        assert!(!names.iter().any(|n| n.contains("build_me_a_playlist")));
    }

    #[test]
    fn a_prompt_the_connection_cannot_use_is_refused_not_rendered() {
        assert!(get(&auth(scopes::LISTENING_READ), "build_me_a_playlist", &json!({})).is_err());
        assert!(get(&auth(scopes::PLAYLISTS_WRITE), "build_me_a_playlist", &json!({})).is_ok());
    }

    #[test]
    fn the_vision_rule_survives_into_the_prompts_themselves() {
        // The rule that nothing recommends unowned music is stated in the
        // server instructions, which a client may or may not surface. A prompt
        // is the last place it can be restated before work starts, so both
        // suggestion prompts carry it.
        for name in ["what_should_i_put_on", "build_me_a_playlist"] {
            let scope = if name == "build_me_a_playlist" {
                scopes::PLAYLISTS_WRITE
            } else {
                scopes::LIBRARY_READ
            };
            let p = get(&auth(scope), name, &json!({})).unwrap();
            let text = p["messages"][0]["content"]["text"].as_str().unwrap().to_lowercase();
            assert!(
                text.contains("already have") || text.contains("never played"),
                "prompt '{name}' does not restate the owned-music rule"
            );
        }
    }

    #[test]
    fn a_missing_argument_falls_back_rather_than_rendering_a_hole() {
        let p = get(&auth(scopes::PLAYLISTS_WRITE), "build_me_a_playlist", &json!({})).unwrap();
        let text = p["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(!text.contains("{brief}"), "unsubstituted placeholder reached the model");
        assert!(text.contains("About 20 tracks"));
    }
}
