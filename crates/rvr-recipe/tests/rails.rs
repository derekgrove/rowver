//! The recipe on rails: every problem said at once, names and arities checked as it is read, one
//! depth limit, truth values only where one is needed, exact comparisons and conversions, and data
//! errors that name their event.

mod common;
use common::{complete, Held};
use rvr_recipe::{parse, Array, Columns, Constant, Kind, Picked, Recipe, Scope, Values, DEEPEST};
use std::collections::BTreeMap;

/// Every problem `body` (after the header) is refused with, one a line.
fn refused(body: &str) -> Vec<String> {
    let said = Recipe::read(&complete(body)).unwrap_err().0;
    said.lines().map(str::to_owned).collect()
}

/// What `text` gives over `columns`, with this sample's metadata.
fn run(columns: &[(&str, Array)], sample: &[(&str, Constant)], text: &str) -> rvr_recipe::Result<Array> {
    let held = Held(columns.iter().map(|(n, a)| (n.to_string(), a.clone())).collect());
    let sample: BTreeMap<String, Constant> = sample.iter().map(|(n, c)| (n.to_string(), c.clone())).collect();
    let defines = BTreeMap::new();
    Scope::new(&held, &sample, &defines).evaluate(&parse(text)?)
}

fn flat(kind: Kind, values: Vec<i64>) -> Array {
    Array::Event(Values::whole(kind, values))
}

// RECIPE the-keys-exactly, mistakes; SPEC 1.4, 1.5, 4.1.1, 4.1.2, 4.1.3, 4.1.4, 4.1.5
#[test]
fn a_recipe_with_many_problems_is_refused_with_all_of_them() {
    let said = refused("titel = \"x\"\n[define]\ngood = \"cnt(Jet_pt > 30)\"\n[keep]\n\"two jets\" = \"count(good) >= 2\"\nmet = { if = \"sample.era == 3\", aply = \"MET_pt > 30\" }\n[columns]\nht = { value = \"sum(Jet_pt, 1)\", stored = \"f2\" }\n[totals]\nsumw = { value = \"genWeight\", trees = \"Runs\" }\n[derived]\nr = \"ht / nothing\"\n");
    let want = [
        "A recipe has no key \"titel\"; it takes format, version, title, define, keep, columns, totals, derived. Did you mean: title?",
        "[define] good: There is no function named cnt. Did you mean: ",
        "[keep] two jets: a cut is named with one plain name, such as two_muons",
        "[keep] met has no key \"aply\"; it takes if, apply. Did you mean: apply?",
        "[keep] met needs apply = \"…\"",
        "[columns] ht stored: \"f2\" is not a number kind: the kinds are b1, i1, i2, i4, i8, u1, u2, u4, u8, f4, f8. Did you mean: f4, f8, i2?",
        "[columns] ht: sum takes 1 value, not 2",
        "[totals] sumw has no key \"trees\"; it takes value, tree. Did you mean: tree?",
        "[derived] r uses nothing, but the file does not store nothing",
    ];
    assert_eq!(said.len(), want.len(), "{said:#?}");
    for (said, want) in said.iter().zip(want) {
        assert!(said.starts_with(want), "{said}\nwanted {want}");
    }
    // A define that could not be read is still one above the lines below it.
    let said = refused("[define]\na = \"1 +\"\nb = \"a * 2\"\n");
    assert_eq!(said, ["[define] a: Expected a value at the end (column 4)"]);
    assert_eq!(refused("title = 3\n"), ["title must be text, as title = \"my study\""]);
    assert_eq!(refused("[columns]\n\"good-pt\" = \"Jet_pt\"\n"), ["[columns] good-pt: a column is named with one plain name, such as ht, and not sample, entry or file"]);
    assert_eq!(refused("[keep]\nx = 3\n"), ["[keep] x must be a condition in quotes, or { if = \"…\", apply = \"…\" }"]);
    // A branch stored as it is has a name every column may have: not sample, entry or file.
    assert_eq!(refused("[columns]\nbranches = [\"Jet_pt\", \"entry\"]\n"), ["[columns] branches lists entry, a name every row has already (sample, entry, file): store the branch under another name, as entry_branch = \"branch('entry')\""]);
    // Every problem on one line, for a sentence that holds them.
    let both = Recipe::read(&complete("titel = 1\nx = 2\n")).unwrap_err();
    assert!(both.0.contains('\n') && !both.line().contains('\n') && both.line().contains("\"titel\"; it takes") && both.line().contains("; A recipe has no key \"x\""), "{}", both.line());
}

// RECIPE mistakes, functions-exactly, defined-names, values, operators, text-branches-and-stored-types; SPEC 1.5, 4.1.5
#[test]
fn functions_and_their_values_are_checked_as_the_recipe_is_read() {
    // The name and number of values of each call, inside out: an argument's own mistake hides none.
    assert_eq!(refused("[columns]\nx = \"count(nope(Jet_pt), 3)\"\n"), [format!("[columns] x: There is no function named nope. Every function, with an example, is in the recipe reference: {}", rvr_recipe::REFERENCE), "[columns] x: count takes 1 value, not 2".to_owned()]);
    assert_eq!(refused("[define]\nfirst = \"Jet_pt\"\n[columns]\nx = \"first(Jet_pt, 0)\"\n"), ["[columns] x: first is a defined name, not a function: write it without brackets, as first"]);
    for (expression, said) in [
        ("nth(Jet_pt, -1, 0)", "nth counts positions from 0"),
        ("nth(Jet_pt, 1.5, 0)", "nth needs a whole number for the position"),
        ("sorted(Jet_pt, 'up')", "sorted takes 'ascending' or 'descending' as its direction, not \"up\""),
        ("stored(Jet_pt, 'f16')", "stored: \"f16\" is not a number kind"),
        ("stored(3, 'f4')", "stored needs a column, not a fixed value"),
        ("shift_right(Flags, -2)", "shift_right cannot shift by a negative number of bits"),
        ("bit_and(Flags, 1.5)", "bit_and needs a whole number as its second value"),
        ("count(sample.n)", "count needs a list per event, not a fixed value"),
        ("least_index(Jet_pt, 0.5)", "least_index needs a whole number to use when there is no member"),
        ("branch(3)", "branch needs a branch name as text"),
        ("mass(Jet_pt, Jet_eta)", "mass takes 3 or 4 values (pt, eta, phi, and optionally a mass), not 2"),
        ("Jet_pt + sample.events_read", "sample.events_read is a count rvr press records of each sample"),
        ("'a' + 1", "Text cannot be combined with a number"),
        ("'a' < 'b'", "Text can only be compared with == and !=, not <"),
    ] {
        let found = refused(&format!("[columns]\nx = \"{expression}\"\n"));
        assert!(found.len() == 1 && found[0].starts_with(&format!("[columns] x: {said}")), "{expression}: {found:?}");
    }
}

// RECIPE operators, two-kinds-together, functions-exactly, choosing-members-of-a-list, the-parts-of-a-recipe, mistakes; SPEC 4.1.5
#[test]
fn truth_values_stand_wherever_one_is_needed() {
    // Refused as the recipe is read where the text tells its kind.
    let fix = "needs a true-or-false value, not a number: to test a number, compare it, as x != 0";
    for (body, place) in [
        ("[keep]\nx = \"count(Jet_pt)\"\n", "[keep] x: A cut's condition"),
        ("[keep]\nx = { if = \"sample.era\", apply = \"2\" }\n", "[keep] x: A cut's condition"),
        ("[columns]\nx = \"not count(Jet_pt)\"\n", "[columns] x: not"),
        ("[columns]\nx = \"MET_pt > 3 and 2\"\n", "[columns] x: and's right side"),
        ("[columns]\nx = \"if(nJet + 1, 1, 0)\"\n", "[columns] x: if's condition"),
        ("[define]\nn = \"count(Jet_pt)\"\n[columns]\nx = \"Jet_pt[n]\"\n", "[columns] x: A mask"),
    ] {
        assert_eq!(refused(body), [format!("{place} {fix}")], "{body}");
    }
    // Else when it is worked out, over the columns' kinds.
    let n = flat(Kind::I4, vec![0, 2]);
    for (expression, said) in [
        ("not nJet", "not needs a true-or-false value, not i4: to test a number, compare it, as x != 0"),
        ("nJet or nJet > 1", "or needs a true-or-false value, not i4"),
        ("if(nJet, 1, 0)", "if's condition needs a true-or-false value, not i4"),
        ("-(nJet > 1)", "- needs a number, not a true-or-false value"),
    ] {
        let error = run(&[("nJet", n.clone())], &[], expression).unwrap_err().0;
        assert!(error.starts_with(said), "{expression}: {error}");
    }
    let held = Held([("nJet".to_owned(), n)].into());
    let (sample, defines) = (BTreeMap::new(), BTreeMap::new());
    let scope = Scope::new(&held, &sample, &defines);
    assert!(scope.mask(&parse("nJet").unwrap(), 2).unwrap_err().0.starts_with("A condition needs a true-or-false value, not i4"));
    assert_eq!(scope.mask(&parse("not (nJet > 1)").unwrap(), 2).unwrap(), [true, false]);
}

// RECIPE values, cuts-for-some-samples-only, functions-exactly; SPEC 4.3.2
#[test]
fn a_comparison_with_a_missing_field_is_false_and_arithmetic_with_one_an_error() {
    let jets = Array::object(&[2, 0], Values::float(Kind::F4, vec![40.0, 20.0])).unwrap();
    let columns = [("Jet_pt", jets), ("MET_pt", Array::Event(Values::float(Kind::F4, vec![1.0, 2.0])))];
    let falses = |text: &str| run(&columns, &[], text).unwrap();
    assert_eq!(falses("MET_pt > sample.cut"), Array::Event(Values::whole(Kind::B1, vec![0, 0])));
    assert_eq!(falses("MET_pt != sample.cut"), Array::Event(Values::whole(Kind::B1, vec![0, 0])));
    assert_eq!(falses("count(Jet_pt > sample.cut)"), Array::Event(Values::whole(Kind::I8, vec![0, 0])));
    assert_eq!(falses("sample.cut in [1, 2]"), Array::Constant(Constant::Truth(false)));
    assert_eq!(falses("contains(sample.kind, 'sig')"), Array::Constant(Constant::Truth(false)));
    assert!(run(&columns, &[], "MET_pt * sample.cut").unwrap_err().0.starts_with("sample.cut is used as a number, but this sample has no cut"));
    assert!(run(&columns, &[], "not sample.vbf").unwrap_err().0.starts_with("sample.vbf is used as a true-or-false value"));
}

// RECIPE kinds-exactly, two-kinds-together
#[test]
fn whole_numbers_compare_exactly() {
    let big = 1_i64 << 53;
    let signed = flat(Kind::I8, vec![big + 1, -1, i64::MAX]);
    let unsigned = flat(Kind::U8, vec![big, -1, i64::MAX]);
    let columns = [("i", signed), ("u", unsigned)];
    // 2^53 + 1 is not 2^53, -1 is less than 2^64 - 1, and i64::MAX is the same in both.
    assert_eq!(run(&columns, &[], "i == u").unwrap(), flat(Kind::B1, vec![0, 0, 1]));
    assert_eq!(run(&columns, &[], "i < u").unwrap(), flat(Kind::B1, vec![0, 1, 0]));
    // Arithmetic between them is in f8, as NumPy's promotion gives.
    assert_eq!(run(&columns, &[], "i + u").unwrap(), Array::Event(Values::float(Kind::F8, vec![(big + 1) as f64 + big as f64, u64::MAX as f64 - 1.0, 2.0 * i64::MAX as f64])));
}

// RECIPE conversions, values-the-recipe-cannot-take, what-differs-from-numpy; SPEC 2.6, 4.4.2
#[test]
fn conversions_truncate_round_once_and_refuse_what_does_not_fit() {
    let floats = |values: Vec<f64>| Values::float(Kind::F8, values);
    let whole = |kind: Kind, values: Vec<i64>| Values::whole(kind, values);
    assert_eq!(floats(vec![2.9, -2.9, -0.5, 2147483647.9]).convert(Kind::I4).unwrap(), whole(Kind::I4, vec![2, -2, 0, i32::MAX as i64]));
    for (value, kind, said) in [
        (2147483648.0, Kind::I4, "2147483648.0 does not fit in i4, which holds -2147483648 to 2147483647"),
        (-2147483649.0, Kind::I4, "-2147483649.0 does not fit in i4"),
        (3e10, Kind::I4, "30000000000.0 does not fit in i4"),
        (f64::NAN, Kind::I4, "NaN is no number, so it has no i4 value"),
        (f64::NAN, Kind::B1, "NaN is neither true nor false, so it has no b1 value"),
        (-1.0, Kind::U1, "-1.0 does not fit in u1, which holds 0 to 255"),
        (f64::INFINITY, Kind::I8, "inf does not fit in i8"),
    ] {
        let error = floats(vec![0.0, 0.0, value]).convert(kind).unwrap_err();
        assert!(error.0.starts_with(said) && error.1 == Some(2), "{value} to {kind:?}: {error:?}");
    }
    assert!(whole(Kind::I8, vec![300]).convert(Kind::U1).unwrap_err().0.starts_with("300 does not fit in u1"));
    assert!(whole(Kind::U8, vec![-1]).convert(Kind::I8).unwrap_err().0.starts_with("18446744073709551615 does not fit in i8"));
    assert_eq!(floats(vec![0.0, -0.0, 0.5]).convert(Kind::B1).unwrap(), whole(Kind::B1, vec![0, 0, 1]));
    assert_eq!(whole(Kind::I4, vec![0, -7]).convert(Kind::B1).unwrap(), whole(Kind::B1, vec![0, 1]));
    // A whole number to a float is rounded once, to nearest, ties to even: straight to f4, not
    // through f8 (which would round 2^60 + 2^36 + 1 twice, to 2^60).
    let odd = (1_i64 << 60) + (1 << 36) + 1;
    let f4 = whole(Kind::I8, vec![(1 << 53) + 1, odd]).convert(Kind::F4).unwrap();
    assert_eq!(f4.numbers(), [((1_i64 << 53) as f32) as f64, (odd as f32) as f64]);
    assert_ne!((odd as f32) as f64, ((odd as f64) as f32) as f64);
    assert_eq!(whole(Kind::I8, vec![(1 << 53) + 1]).convert(Kind::F8).unwrap().numbers(), [(1_i64 << 53) as f64]);
    assert_eq!(whole(Kind::U8, vec![-1]).convert(Kind::F4).unwrap().numbers(), [u64::MAX as f32 as f64]);
    // A value of a list names the event it is in.
    let lists = Array::object(&[1, 0, 2], floats(vec![1.0, 2.0, 1e10])).unwrap();
    assert_eq!(lists.convert(Kind::I4).unwrap_err().1, Some(2));
}

// RECIPE values-the-recipe-cannot-take, choosing-members-of-a-list, pairs-within-a-list, numbers, functions-exactly; SPEC 2.6
#[test]
fn a_value_the_expression_cannot_take_is_a_data_error_of_its_event() {
    let jets = Array::object(&[1, 2, 3], Values::float(Kind::F4, vec![1.0; 6])).unwrap();
    let mask = Array::object(&[1, 3, 3], Values::whole(Kind::B1, vec![1; 7])).unwrap();
    let many = Array::object(&[2, 362, 363], Values::float(Kind::F8, vec![1.0; 727])).unwrap();
    let columns = [("Jet_pt", jets), ("good", mask), ("track", many)];
    let error = run(&columns, &[], "Jet_pt[good]").unwrap_err();
    assert_eq!(error.1, Some(1));
    assert!(error.0.starts_with("A mask has one true-or-false value per member of the list it picks from, and in this event one has 3 members and the other 2"), "{error:?}");
    assert_eq!(run(&columns, &[], "Jet_pt + good").unwrap_err().1, Some(1));
    assert_eq!(run(&columns, &[], "pt(Jet_pt, good, Jet_pt)").unwrap_err().1, Some(1));
    // The pair limit is one event's: 362 members make 65,341 pairs, 363 make 65,703.
    let error = run(&columns, &[], "pair_first(track)").unwrap_err();
    assert_eq!(error.1, Some(2));
    assert!(error.0.starts_with("pair_first would make 65703 pairs of this event's 363 members, more than the 65536 one event may make"), "{error:?}");
    assert!(run(&columns, &[], "pair_dr(track[index(track) < 362], track[index(track) < 362])").is_ok());
    let n = flat(Kind::I4, vec![2, -1]);
    assert_eq!(run(&[("n", n)], &[], "n ** n").unwrap_err().1, Some(1));
    let charges = Array::object(&[2, 0, 1], Values::whole(Kind::I4, vec![1, 1, -1])).unwrap();
    assert_eq!(run(&[("q", charges)], &[], "2 ** q").unwrap_err().1, Some(2));
}

// RECIPE how-deep; SPEC 4.1.5
#[test]
fn every_recursive_construct_counts_toward_one_depth_limit() {
    let deepest = |open: &str, close: &str, levels: usize| format!("{}x{}", open.repeat(levels - 1), close.repeat(levels - 1));
    for (open, close) in [("(", ")"), ("-", ""), ("not ", ""), ("abs(", ")"), ("x[", "]"), ("x in [", "]"), ("x ** ", "")] {
        let last = deepest(open, close, DEEPEST);
        let held = Held([("x".to_owned(), Array::Event(Values::whole(Kind::B1, vec![1])))].into());
        let (sample, defines) = (BTreeMap::new(), BTreeMap::new());
        // The deepest is read and worked out on a thread of the stack a press's workers have.
        let expression = parse(&last).unwrap_or_else(|e| panic!("{open}: {e}"));
        std::thread::Builder::new().stack_size(2 << 20).spawn(move || drop(Scope::new(&held, &sample, &defines).evaluate(&expression))).unwrap().join().unwrap();
        let past = parse(&deepest(open, close, DEEPEST + 1)).unwrap_err().0;
        assert!(past.starts_with("The expression is more than 100 levels deep"), "{open}: {past}");
        // Far past it, refused just the same, not by a stack overflow.
        assert!(parse(&deepest(open, close, 100_000)).is_err());
    }
    let chain = vec!["x"; DEEPEST].join(" + ");
    assert!(parse(&chain).is_ok() && parse(&format!("{chain} + x")).is_err());
    assert!(parse(&vec!["x"; 200_000].join(" * ")).is_err());
    // A defined name is as deep as its expression.
    let half = deepest("-", "", 60);
    assert!(Recipe::read(&complete(&format!("[define]\na = \"{half}\"\n[columns]\nb = \"{half}\"\n"))).is_ok());
    let said = refused(&format!("[define]\na = \"{half}\"\n[columns]\nb = \"{}a\"\n", "-".repeat(50)));
    assert!(said.len() == 1 && said[0].starts_with("[columns] b: The expression is more than 100 levels deep"), "{said:?}");
    // An expression read beside a recipe, as rvr try reads one, counts its defined names alike.
    let read = Recipe::read(&complete(&format!("[define]\na = \"{half}\"\n"))).unwrap();
    assert!(read.expression(&format!("{}a", "-".repeat(30))).is_ok());
    let deep = read.expression(&format!("{}a", "-".repeat(50))).unwrap_err().0;
    assert!(deep.starts_with("The expression is more than 100 levels deep"), "{deep}");
    // Its brackets count as the recipe counts them when it is read.
    let bracketed = format!("{}x{}", "(".repeat(59), ")".repeat(59));
    let read = Recipe::read(&complete(&format!("[define]\na = \"{bracketed}\"\n"))).unwrap();
    assert!(read.expression(&format!("{}a", "-".repeat(40))).is_ok());
    assert!(read.expression(&format!("{}a", "-".repeat(41))).is_err());
}

// RECIPE signed-zeros, numbers, operators, functions-exactly
#[test]
fn signed_zeros_are_numpys() {
    let x = Array::Event(Values::float(Kind::F8, vec![-4.0, 4.0, -0.0, 0.0]));
    let bits = |text: &str| match run(&[("x", x.clone())], &[], text).unwrap() {
        Array::Event(values) => values.numbers().iter().map(|v| v.is_sign_negative()).collect::<Vec<_>>(),
        other => panic!("{other:?}"),
    };
    // A zero remainder takes the divisor's sign.
    assert_eq!(bits("x % 2"), [false, false, false, false]);
    assert_eq!(bits("x % -2"), [true, true, true, true]);
    assert_eq!(bits("sign(x)"), [true, false, false, false]);
    // min and max give the right one of two equal values.
    assert_eq!(bits("min(-x, x)"), [true, true, true, false]);
    assert_eq!(bits("max(x, -x)"), [false, false, false, true]);
    assert_eq!(bits("-x"), [false, true, false, true]);
}

// RECIPE functions-exactly, pairs-within-a-list; FORMAT 10
#[test]
fn events_whose_pairs_are_too_many_together_are_worked_out_in_runs_of_fewer() {
    // 1,028 events of 362 members each, every one under the limit, make 67,170,548 pairs together,
    // more than one run may hold (2^26): worked out at once that is said, never a data error.
    let events = 1028;
    let lists = Array::object(&vec![362; events], Values::whole(Kind::I4, (0..events as i64 * 362).collect())).unwrap();
    let error = run(&[("x", lists.clone())], &[], "count(pair_first(x) > 0)").unwrap_err();
    assert!(error.crowded() && error.1.is_none(), "{error:?}");
    // In halves, as a press and a reader work them, each run holds fewer and gives the same values.
    let held = Held([("x".to_owned(), lists)].into());
    let (sample, defines) = (BTreeMap::new(), BTreeMap::new());
    let expression = parse("count(pair_first(x) > 0)").unwrap();
    let mut runs = 0;
    let parts = rvr_recipe::in_runs(0, events, &mut |from, to| {
        runs += 1;
        let part = Held(held.0.iter().map(|(n, a)| (n.clone(), a.rows(from, to))).collect());
        Scope::new(&part, &sample, &defines).evaluate(&expression)
    })
    .unwrap();
    let whole = Array::joined(parts).unwrap();
    assert_eq!(runs, 3, "one too many, then two halves");
    // The first member of each pair is greater than 0 but for the pairs of the very first member.
    let want: Vec<i64> = (0..events).map(|e| if e == 0 { 65_341 - 361 } else { 65_341 }).collect();
    assert_eq!(whole, flat(Kind::I8, want));
}

// RECIPE functions-exactly
#[test]
fn runs_are_joined_whatever_their_lists_begin_at() {
    // A run whose lists begin past the start of its values, and hold fewer than all of them.
    let first = Array::Object { offsets: std::sync::Arc::new(vec![2, 3, 5]), values: Values::whole(Kind::I4, vec![9, 9, 1, 2, 3, 9]) };
    let second = Array::object(&[0, 1], Values::whole(Kind::I4, vec![4])).unwrap();
    let joined = Array::joined(vec![first, second]).unwrap();
    assert_eq!(joined, Array::object(&[1, 2, 0, 1], Values::whole(Kind::I4, vec![1, 2, 3, 4])).unwrap());
    assert!(Array::joined(vec![flat(Kind::I4, vec![1]), flat(Kind::I8, vec![2])]).is_err());
}

/// A flat column `x` and a list column `l` over four rows.
fn four() -> Held {
    let x = Array::Event(Values::float(Kind::F8, vec![0.0, 1.0, 2.0, 3.0]));
    let l = Array::object(&[1, 0, 2, 1], Values::whole(Kind::I4, vec![10, 20, 30, 40])).unwrap();
    Held(BTreeMap::from([("x".to_owned(), x), ("l".to_owned(), l)]))
}

// SPEC 4.2.2
#[test]
fn picked_rows_are_the_rows_at_their_positions_and_every_row_is_the_columns_themselves() {
    let held = four();
    let picked = Picked::new(&held, &[1, 2], 4);
    assert_eq!(picked.column("x").unwrap(), Some(Array::Event(Values::float(Kind::F8, vec![1.0, 2.0]))));
    assert_eq!(picked.column("l").unwrap(), Some(Array::object(&[0, 2], Values::whole(Kind::I4, vec![20, 30])).unwrap()));
    assert_eq!(picked.column("y").unwrap(), None);
    let every = Picked::new(&held, &[0, 1, 2, 3], 4);
    assert_eq!(every.column("l").unwrap(), four().column("l").unwrap());
}

// RECIPE defined-names, mistakes
#[test]
fn a_missing_column_and_a_defined_name_a_column_has_are_told_by_kind_not_words() {
    let defines = BTreeMap::from([("x".to_owned(), parse("1").unwrap())]);
    let sample = BTreeMap::new();
    let held = four();
    let scope = Scope::new(&held, &sample, &defines);
    let missing = scope.evaluate(&parse("y + 1").unwrap()).unwrap_err().at("[columns] z");
    assert_eq!((missing.missing(), missing.clash()), (Some("y"), false));
    let shadowed = BTreeMap::from([("l".to_owned(), parse("1").unwrap())]);
    let clash = Scope::new(&held, &sample, &shadowed).evaluate(&parse("l").unwrap()).unwrap_err();
    assert!(clash.clash() && clash.missing().is_none());
    assert_eq!(missing.of_event(Some(3)).missing(), Some("y"));
}

// RECIPE defined-names
#[test]
fn a_defined_name_worked_out_in_one_scope_is_known_to_another() {
    let defines = BTreeMap::from([("big".to_owned(), parse("x > 1").unwrap())]);
    let sample = BTreeMap::new();
    let held = four();
    let cuts = Scope::new(&held, &sample, &defines);
    cuts.evaluate(&parse("big").unwrap()).unwrap();
    let known = cuts.worked_out();
    assert_eq!(known.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["big"]);
    // Given another value than its expression's, the other scope takes it as worked out.
    let seeded = vec![("big".to_owned(), Array::Event(Values::whole(Kind::B1, vec![1, 1, 1, 1])))];
    let columns = Scope::new(&held, &sample, &defines).knowing(seeded);
    assert_eq!(columns.evaluate(&parse("big").unwrap()).unwrap(), Array::Event(Values::whole(Kind::B1, vec![1, 1, 1, 1])));
}

// RECIPE conversions
#[test]
fn a_promotion_that_does_not_fit_is_refused_not_a_panic() {
    assert!(Values::float(Kind::F8, vec![f64::NAN]).convert(Kind::I4).is_err());
    assert!(Values::whole(Kind::I8, vec![300]).convert(Kind::U1).is_err());
    assert_eq!(Values::whole(Kind::B1, vec![1]).convert(Kind::I8).unwrap(), Values::whole(Kind::I8, vec![1]));
}

// RECIPE operators, values, mistakes; SPEC 1.5, 4.1.5
#[test]
fn a_slip_of_syntax_says_what_to_write() {
    let said = |text: &str| parse(text).unwrap_err().0;
    assert!(said("a < b < c").contains("write a < b and b < c"));
    assert!(said("a == not b").contains("write a == (not b)"));
    assert!(said("a = 1").contains("write =="));
    assert!(said("!a").contains("write not"));
    assert!(said("a && b").starts_with("Unexpected && at column 3: write and"));
    assert!(said("a | b").contains("write or"));
    assert!(said("+x").contains("there is no unary +"));
    assert!(said("9223372036854775808").contains("the largest whole number, 9223372036854775807"));
    assert!(parse("9_223_372_036_854_775_807 + 0.000_1").is_ok());
    // Both sides of and are checked at once.
    let both = refused("[keep]\nk = \"1 and 2\"\n");
    assert!(both.len() == 2 && both[0].contains("and's left side") && both[1].contains("and's right side"), "{both:?}");
    // A field the sample lacks is said as one, wherever it stands.
    let lists = Array::object(&[2], Values::float(Kind::F4, vec![1.0, 2.0])).unwrap();
    let columns = [("l", lists), ("n", flat(Kind::I4, vec![3]))];
    for text in ["nth(l, sample.k, 0)", "bit_and(n, sample.k)", "nearest_index(l, l, l, l, sample.k)"] {
        assert!(run(&columns, &[], text).unwrap_err().0.contains("this sample has no k"), "{text}");
    }
    assert!(Array::joined(Vec::new()).is_err());
}
