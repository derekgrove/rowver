//! `samples.toml`: what a study reads, as a person writes it. Expanded here into one plan per
//! sample, before anything is looked up.
//!
//! ```toml
//! [places]                 # short names for long locations, used as {lpc}
//! lpc = "root://cmseos.fnal.gov//store/user/username/ntuples"
//!
//! [defaults]               # every sample has these unless it says otherwise
//! tree = "KUAnalysis"
//!
//! [eras.UL18]              # a sample that lists eras becomes one sample per era;
//! era = 3                  # an era's values become metadata and can fill placeholders
//!
//! [[sample]]
//! name = "ST_s-channel"    # letters, digits, _ and -: what every tool calls it
//! label = "ST s-channel"   # any text: what plots show (the name when left out)
//! files = "{lpc}/Summer20{era}_106X/ST_s-channel_4f_leptonDecays_TuneCP5_13TeV-amcatnlo-pythia8_Summer20{era}_106X.root"
//! eras = ["UL18"]          # makes ST_s-channel_UL18, labelled "ST s-channel (UL18)"
//! kind = "background"      # any other key is metadata
//! ```

use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// Keys of a sample that name it or say where its events are; every other key is metadata.
pub const SOURCE_KEYS: [&str; 9] = [
    "name", "label", "files", "tree", "trees", "eras", "events", "choose", "seed",
];

pub use rowver::choice::Events;

/// The `events`, `choose` and `seed` of a sample (or of [defaults]).
pub fn events_of(table: &toml::Table, place: &str) -> Result<Events, String> {
    let n = match table.get("events") {
        None => None,
        Some(toml::Value::String(all)) if all == "all" => None,
        Some(toml::Value::Integer(n)) if *n > 0 => Some(*n as u64),
        Some(_) => {
            return Err(problem(format!(
                "{place}: events is a number of events to read, like events = 50_000, or \"all\""
            )))
        }
    };
    let seed = match table.get("seed") {
        None => 1,
        Some(toml::Value::Integer(s)) if *s >= 0 => *s as u64,
        Some(_) => {
            return Err(problem(format!(
                "{place}: seed is a whole number, like seed = 7"
            )))
        }
    };
    match (table.get("choose").map(|c| c.as_str()), n) {
        (None | Some(Some("first")), None) => Ok(Events::All),
        (None | Some(Some("first")), Some(n)) => Ok(Events::First(n)),
        (Some(Some("random")), Some(n)) => Ok(Events::Random { n, seed }),
        (Some(Some("random")), None) => Err(problem(format!("{place}: choose = \"random\" needs a number of events to choose, like events = 50_000"))),
        _ => Err(problem(format!("{place}: choose is \"first\" (the first entries, in file order; the default) or \"random\""))),
    }
}

/// Where one sample's events are, before it is resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// Paths, URLs or patterns (`*` and `?` in the file name).
    Files(Vec<String>),
}

/// One sample as it will be read.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// What every tool calls it: letters, digits, `_` and `-`.
    pub name: String,
    /// What plots and the viewer show.
    pub label: String,
    /// The name of the [[sample]] entry it came from, before eras and trees were added.
    pub from: String,
    /// Which [[sample]] entry, counting from 1.
    pub entry: usize,
    pub source: Source,
    pub tree: String,
    pub events: Events,
    pub metadata: Map<String, Value>,
}

/// A problem in samples.toml, said so that the person who wrote it can fix it.
pub fn problem(text: impl Into<String>) -> String {
    text.into()
}

/// Whether text can be a name: letters, digits, `_` and `-`, at least one character.
pub fn is_name(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Text made into a name, as CMS dataset names write numbers: `.` becomes `p`, every other run of
/// characters that a name cannot hold becomes one `_`, and `_` is trimmed from both ends.
/// "300/292.5" is "300_292p5"; "ST s-channel (13.6 TeV)" is "ST_s-channel_13p6_TeV".
pub fn as_name(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            out.push(c);
        } else if c == '.' {
            out.push('p');
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

/// Fill `{name}` placeholders from `vars`; an unknown one is an error naming those known.
pub fn fill(text: &str, vars: &Map<String, Value>, place: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..]
            .find('}')
            .ok_or_else(|| problem(format!("{place}: a '{{' is never closed in {text:?}")))?;
        let key = &rest[open + 1..open + close];
        match vars.get(key) {
            Some(Value::String(s)) => out.push_str(s),
            Some(other) => out.push_str(&other.to_string()),
            None => {
                let known: Vec<&str> = vars.keys().map(String::as_str).collect();
                return Err(problem(format!(
                    "{place}: {{{key}}} is not defined here. Defined: {}.",
                    if known.is_empty() {
                        "nothing".to_string()
                    } else {
                        known
                            .iter()
                            .map(|k| format!("{{{k}}}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                )));
            }
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn table<'a>(doc: &'a toml::Table, key: &str) -> Result<Option<&'a toml::Table>, String> {
    match doc.get(key) {
        None => Ok(None),
        Some(toml::Value::Table(t)) => Ok(Some(t)),
        Some(_) => Err(problem(format!(
            "[{key}] must be a table of names and values"
        ))),
    }
}

fn json(value: &toml::Value) -> Value {
    serde_json::to_value(value).expect("TOML values are JSON values")
}

/// Every sample samples.toml describes, in the order written (eras, then trees, inside an entry).
pub fn expand(text: &str) -> Result<Vec<Plan>, String> {
    let doc: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| problem(format!("samples.toml is not valid TOML: {e}")))?;
    for key in doc.keys() {
        if ![
            "format", "version", "title", "places", "defaults", "eras", "sample",
        ]
        .contains(&key.as_str())
        {
            return Err(problem(format!(
                "samples.toml has a top-level key {key:?}; the known ones are format, version, title, [places], [defaults], [eras] and [[sample]]"
            )));
        }
    }
    let mut vars = Map::new();
    for (name, value) in table(&doc, "places")?.into_iter().flatten() {
        let toml::Value::String(place) = value else {
            return Err(problem(format!(
                "[places] {name} must be text: a folder, a path or a URL"
            )));
        };
        vars.insert(
            name.clone(),
            Value::String(place.trim_end_matches('/').to_string()),
        );
    }
    let defaults = table(&doc, "defaults")?.cloned().unwrap_or_default();
    let eras = table(&doc, "eras")?.cloned().unwrap_or_default();
    for era in eras.keys() {
        if !is_name(era) {
            return Err(problem(format!("the era {era:?} must be a name (letters, digits, _ and -), since it becomes part of sample names. Try {:?}.", as_name(era))));
        }
    }
    let entries = match doc.get("sample") {
        None => Vec::new(),
        Some(toml::Value::Array(list)) => list.clone(),
        Some(_) => return Err(problem("samples are written [[sample]], with two brackets: one [[sample]] per entry, each with its own name = \"…\".")),
    };
    let mut plans = Vec::new();
    for (index, body) in entries.iter().enumerate() {
        let toml::Value::Table(body) = body else {
            return Err(problem(format!("sample {} must be a table", index + 1)));
        };
        let base = match body.get("name") {
            Some(toml::Value::String(n)) => n.clone(),
            Some(_) => return Err(problem(format!("sample {}: name must be text, like name = \"ST_s-channel\"", index + 1))),
            None => return Err(problem(format!("sample {} has no name. Give it one, like name = \"ST_s-channel\" (and label = \"ST s-channel\" for plots).", index + 1))),
        };
        let place = format!("sample {base:?}");
        if !is_name(&base) {
            return Err(problem(format!(
                "{place}: a name holds only letters, digits, _ and -, since tools use it in commands and file names. Use name = {:?}, and put the text in label = {base:?}.",
                as_name(&base)
            )));
        }
        let label = match body.get("label") {
            Some(toml::Value::String(l)) => l.clone(),
            Some(_) => return Err(problem(format!("{place}: label must be text"))),
            None => base.clone(),
        };
        let mut merged = defaults.clone();
        merged.extend(body.clone());
        let era_list: Vec<Option<String>> = match merged.get("eras") {
            None => vec![None],
            Some(toml::Value::Array(list)) => list
                .iter()
                .map(|e| e.as_str().map(|s| Some(s.to_string())))
                .collect::<Option<_>>()
                .ok_or_else(|| problem(format!("{place}: eras must be a list of era names")))?,
            Some(_) => {
                return Err(problem(format!(
                    "{place}: eras must be a list of era names, like eras = [\"UL17\", \"UL18\"]"
                )))
            }
        };
        let trees: Vec<(Option<String>, String)> = match merged.get("trees") {
            Some(toml::Value::Table(points)) => points.iter().map(|(point, tree)| tree.as_str().map(|t| (Some(point.clone()), t.to_string())))
                .collect::<Option<_>>().ok_or_else(|| problem(format!("{place}: trees maps a label to a tree name, like trees = {{ \"300/250\" = \"SMS_30000_25000\" }}")))?,
            Some(_) => return Err(problem(format!("{place}: trees maps a label to a tree name, like trees = {{ \"300/250\" = \"SMS_30000_25000\" }}"))),
            None => match merged.get("tree") {
                Some(toml::Value::String(t)) => vec![(None, t.clone())],
                Some(_) => return Err(problem(format!("{place}: tree must be the name of a tree, like tree = \"Events\""))),
                None => return Err(problem(format!("{place}: no tree. Say which tree to read, tree = \"Events\", here or in [defaults]."))),
            },
        };
        let events = events_of(&merged, &place)?;
        let source_of = |vars: &Map<String, Value>| -> Result<Source, String> {
            match merged.get("files") {
                None => Err(problem(format!("{place}: no files. Say where its events are: files = \"…\" (paths, URLs or patterns)."))),
                Some(toml::Value::String(f)) => Ok(Source::Files(vec![fill(f, vars, &place)?])),
                Some(toml::Value::Array(list)) => list.iter().map(|f| f.as_str().ok_or_else(|| problem(format!("{place}: files must be text or a list of text"))).and_then(|f| fill(f, vars, &place)))
                    .collect::<Result<_, _>>().map(Source::Files),
                Some(_) => Err(problem(format!("{place}: files must be text or a list of text"))),
            }
        };
        for era in &era_list {
            let mut vars = vars.clone();
            let mut metadata = Map::new();
            if let Some(era) = era {
                let toml::Value::Table(values) = eras.get(era).ok_or_else(|| {
                    let known: Vec<&String> = eras.keys().collect();
                    problem(format!(
                        "{place}: era {era:?} is not in [eras]. Known eras: {known:?}."
                    ))
                })?
                else {
                    return Err(problem(format!("[eras.{era}] must be a table")));
                };
                for (k, v) in values {
                    vars.insert(k.clone(), json(v));
                    metadata.insert(k.clone(), json(v));
                }
                // {era} is always the era's name, even when the era has a value called era.
                vars.insert("era".into(), Value::String(era.clone()));
            }
            // {file} is the name of the sample's one file, known only once it is resolved.
            let mut later = vars.clone();
            later.insert("file".into(), Value::String("{file}".into()));
            for (key, value) in &merged {
                if !SOURCE_KEYS.contains(&key.as_str()) {
                    let value = match value {
                        toml::Value::String(s) => Value::String(fill(s, &later, &place)?),
                        other => json(other),
                    };
                    metadata.insert(key.clone(), value);
                }
            }
            let source = source_of(&vars)?;
            for (point, tree) in &trees {
                let (mut name, mut shown) = (base.clone(), label.clone());
                if let Some(point) = point {
                    name = format!("{name}_{}", as_name(point));
                    shown = format!("{shown} {point}");
                }
                if let Some(era) = era {
                    name = format!("{name}_{era}");
                    shown = format!("{shown} ({era})");
                }
                plans.push(Plan {
                    name,
                    label: shown,
                    from: base.clone(),
                    entry: index + 1,
                    source: source.clone(),
                    tree: tree.clone(),
                    events,
                    metadata: metadata.clone(),
                });
            }
        }
    }
    let (mut names, mut labels) = (BTreeSet::new(), BTreeSet::new());
    for plan in &plans {
        if !names.insert(plan.name.clone()) {
            return Err(problem(format!(
                "two samples would both be called {:?}. Give one of them another name, or different eras or trees.",
                plan.name
            )));
        }
        if !labels.insert(plan.label.clone()) {
            return Err(problem(format!(
                "two samples would both be labelled {:?}, so plots could not tell them apart. Give one of them another label.",
                plan.label
            )));
        }
    }
    if plans.is_empty() {
        return Err(problem("samples.toml describes no sample. Add one: [[sample]] with name = \"…\" and files = \"…\"."));
    }
    Ok(plans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eras_and_trees_make_one_sample_each() {
        let plans = expand(
            r#"
            [places]
            lpc = "root://cmseos.fnal.gov//store/x/"
            [defaults]
            tree = "KUAnalysis"
            [eras.UL17]
            folder = "Summer20UL17_106X"
            era = 2
            [eras.UL18]
            folder = "Summer20UL18_106X"
            era = 3
            [[sample]]
            name = "Signal"
            files = "{lpc}/{folder}_SMS/SMS-Signal_{folder}.root"
            eras = ["UL17", "UL18"]
            trees = { "300/250" = "SMS_30000_25000", "300/292.5" = "SMS_30000_29250" }
            kind = "signal"
        "#,
        )
        .unwrap();
        let names: Vec<&str> = plans.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Signal_300_250_UL17",
                "Signal_300_292p5_UL17",
                "Signal_300_250_UL18",
                "Signal_300_292p5_UL18"
            ]
        );
        let labels: Vec<&str> = plans.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Signal 300/250 (UL17)",
                "Signal 300/292.5 (UL17)",
                "Signal 300/250 (UL18)",
                "Signal 300/292.5 (UL18)"
            ]
        );
        assert_eq!(plans[2].source, Source::Files(vec!["root://cmseos.fnal.gov//store/x/Summer20UL18_106X_SMS/SMS-Signal_Summer20UL18_106X.root".into()]));
        assert_eq!(plans[2].tree, "SMS_30000_25000");
        assert_eq!(plans[2].metadata["era"], 3);
        assert_eq!(plans[2].metadata["kind"], "signal");
        assert!(!plans[2].metadata.contains_key("label"));
    }

    #[test]
    fn a_label_is_shown_and_the_name_is_used() {
        let plans = expand("[eras.UL18]\nera = 3\n[[sample]]\nname = \"ST_s-channel\"\nlabel = \"ST s-channel\"\ntree = \"E\"\nfiles = \"/x/Summer20{era}_106X.root\"\neras = [\"UL18\"]").unwrap();
        assert_eq!(
            (plans[0].name.as_str(), plans[0].label.as_str()),
            ("ST_s-channel_UL18", "ST s-channel (UL18)")
        );
        assert_eq!(
            plans[0].source,
            Source::Files(vec!["/x/Summer20UL18_106X.root".into()])
        );
        assert_eq!(plans[0].metadata["era"], 3);
    }

    #[test]
    fn names_are_made_as_cms_makes_them() {
        assert_eq!(as_name("300/292.5"), "300_292p5");
        assert_eq!(
            as_name("SignalModel LSP 220 (13.6 TeV Summer23BPix)"),
            "SignalModel_LSP_220_13p6_TeV_Summer23BPix"
        );
        assert_eq!(as_name("ST s-channel"), "ST_s-channel");
        assert!(is_name("ST_s-channel_UL18") && !is_name("ST s-channel") && !is_name(""));
    }

    #[test]
    fn events_choose_and_seed() {
        let plans = expand("[defaults]\ntree = \"E\"\nevents = 50_000\n[[sample]]\nname = \"a\"\nfiles = \"x\"\n[[sample]]\nname = \"b\"\nfiles = \"y\"\nevents = \"all\"\n[[sample]]\nname = \"c\"\nfiles = \"z\"\nchoose = \"random\"\nseed = 7").unwrap();
        assert_eq!(
            plans.iter().map(|p| p.events).collect::<Vec<_>>(),
            [
                Events::First(50_000),
                Events::All,
                Events::Random { n: 50_000, seed: 7 }
            ]
        );
        assert!(!plans[2].metadata.contains_key("seed"));
        assert!(expand(
            "[[sample]]\nname = \"a\"\ntree = \"E\"\nfiles = \"x\"\nchoose = \"random\""
        )
        .unwrap_err()
        .contains("needs a number of events"));
        assert!(
            expand("[[sample]]\nname = \"a\"\ntree = \"E\"\nfiles = \"x\"\nevents = -3")
                .unwrap_err()
                .contains("number of events")
        );
    }

    #[test]
    fn mistakes_are_said_plainly() {
        let err = |text: &str| expand(text).unwrap_err();
        assert!(err("[[sample]]\nname = \"a\"\nfiles = \"x.root\"").contains("no tree"));
        assert!(err("[[sample]]\nname = \"a\"\ntree = \"Events\"").contains("no files"));
        assert!(err("[[sample]]\ntree = \"Events\"\nfiles = \"x.root\"").contains("has no name"));
        assert!(
            err("[[sample]]\nname = \"ST s-channel\"\ntree = \"E\"\nfiles = \"x\"")
                .contains("Use name = \"ST_s-channel\"")
        );
        assert!(err("[sample.a]\ntree = \"E\"\nfiles = \"x\"").contains("two brackets"));
        assert!(
            err("[[sample]]\nname = \"a\"\ntree = \"Events\"\nfiles = \"{lcp}/x.root\"")
                .contains("{lcp} is not defined here")
        );
        assert!(err(
            "[[sample]]\nname = \"a\"\ntree = \"Events\"\nfiles = \"x.root\"\neras = [\"UL19\"]"
        )
        .contains("era \"UL19\" is not in [eras]"));
        assert!(err("[defaults]\ntree = \"E\"\n[[sample]]\nname = \"b\"\nfiles = \"x\"\n[[sample]]\nname = \"b\"\nfiles = \"y\"").contains("both be called \"b\""));
        assert!(err("[defaults]\ntree = \"E\"\n[[sample]]\nname = \"a\"\nlabel = \"A\"\nfiles = \"x\"\n[[sample]]\nname = \"b\"\nlabel = \"A\"\nfiles = \"y\"").contains("both be labelled \"A\""));
    }
}
