//! The known names nearest to a name that is not one, for "Did you mean" in a refusal.

/// The `names` (at most 3, nearest first) within a tenth of `asked`'s length in edits, and at least 2.
pub fn nearest<'a>(asked: &str, names: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let limit = (asked.chars().count() / 10).max(2);
    let mut ranked: Vec<(usize, &str)> = names
        .map(|name| (edits(asked, name), name))
        .filter(|(distance, _)| *distance <= limit)
        .collect();
    ranked.sort();
    ranked.dedup();
    ranked.into_iter().take(3).map(|(_, n)| n).collect()
}

/// " Did you mean: …?" with the `names` nearest to `asked`, or nothing.
pub fn meant<'a>(asked: &str, names: impl Iterator<Item = &'a str>) -> String {
    let near = nearest(asked, names);
    match near.is_empty() {
        true => String::new(),
        false => format!(" Did you mean: {}?", near.join(", ")),
    }
}

/// "`place` has no key `key`", with what it takes, and the known key a slip of a letter or two away.
pub fn unknown(place: &str, key: &str, known: &[&str]) -> String {
    format!("{place} has no key {key:?}; it takes {}.{}", known.join(", "), meant(key, known.iter().copied()))
}

/// Levenshtein distance, by characters.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = (previous + usize::from(ca != *cb)).min(row[j] + 1).min(here + 1);
            previous = here;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC 1.4; RECIPE the-keys-exactly
    #[test]
    fn near_names_within_two_edits_or_a_tenth_of_the_length() {
        let names = ["choose", "tree", "trees", "files", "events"];
        assert_eq!(nearest("choice", names.into_iter()), ["choose"]);
        assert_eq!(nearest("tress", names.into_iter()), ["trees", "tree"]);
        let files = ["ST_s-channel_4f_leptonDecays_TuneCP5_13TeV-amcatnlo-pythia8_Summer20UL18_106X.root", "TTJets.root"];
        let typed = "ST_s-chanel_4f_leptonDecays_TuneCP5_13TeV-amcatnlo-pythia8_Summer20UL18_106X.root";
        assert_eq!(nearest(typed, files.into_iter()), [files[0]]);
        assert!(nearest("completely_different_name.root", files.into_iter()).is_empty());
        let keys = ["name", "label", "files", "tree", "trees", "eras", "events", "choose", "seed", "metadata"];
        for slip in ["kind", "xsec", "lumi"] {
            assert!(nearest(slip, keys.into_iter()).is_empty(), "{slip}");
        }
        assert_eq!(meant("kep", ["keep", "keep"].into_iter()), " Did you mean: keep?");
        assert_eq!(meant("zzzzzz", ["keep"].into_iter()), "");
    }
}
