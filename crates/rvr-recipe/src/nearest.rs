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
    distance(a, b, false)
}

/// The edits from `a` to `b`, by characters, two neighbouring characters swapped counting as one
/// (the optimal string alignment distance): how far a key is a slip of another (FORMAT.md §11).
pub fn slips(a: &str, b: &str) -> usize {
    distance(a, b, true)
}

/// The edits from `a` to `b` (insertions, deletions and replacements, and with `swaps` swaps of
/// two neighbours), each one.
fn distance(a: &str, b: &str, swaps: bool) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    // Rows i - 2, i - 1 and i of the table of distances between prefixes.
    let mut before: Vec<usize> = Vec::new();
    let mut last: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut row = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            row[j] = (last[j - 1] + usize::from(a[i - 1] != b[j - 1])).min(row[j - 1] + 1).min(last[j] + 1);
            if swaps && i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                row[j] = row[j].min(before[j - 2] + 1);
            }
        }
        before = std::mem::replace(&mut last, row);
    }
    last[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    // FORMAT 11
    #[test]
    fn a_slip_counts_a_swap_of_neighbours_as_one_edit() {
        assert_eq!((slips("dervied", "derived"), edits("dervied", "derived")), (1, 2));
        assert_eq!((slips("mde", "made"), slips("data", "data"), slips("", "abc")), (1, 0, 3));
        assert_eq!((slips("dash", "data"), slips("router", "rowver"), slips("driver", "derived"), slips("signal", "signature")), (2, 2, 2, 4));
        // A swap is one edit only of neighbours as they stand: ca to abc is three.
        assert_eq!(slips("ca", "abc"), 3);
    }

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
