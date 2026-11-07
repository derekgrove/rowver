//! The named operations of an expression: reductions over each event's list, picks, and
//! kinematics (the kinematic kernels are hep-columns'). The README's "Functions, exactly" defines
//! each to the bit.

use crate::array::{adopt, lacking, not_truth, together, unlike, Array, Constant, Store, Values};
use crate::kind::Kind;
use crate::{data, problem, Result};
use hep_columns::vectors::Momentum;
use std::sync::Arc;

/// Split a list-per-event array into its offsets and values, or refuse by name.
fn lists<'a>(array: &'a Array, what: &str) -> Result<(&'a Arc<Vec<usize>>, &'a Values)> {
    match array {
        Array::Object { offsets, values } => Ok((offsets, values)),
        Array::Constant(_) => Err(problem(format!("{what} needs a list per event, and is given a fixed value, the same for every event: a sample's metadata, say, or an and or or a fixed value decides (false and X is false, whatever X is)"))),
        _ => Err(problem(format!("{what} needs a list per event"))),
    }
}

/// Apply a float function to every value, in the float kind of the input.
pub(crate) fn float_map(array: &Array, function: fn(f64) -> f64) -> Result<Array> {
    array.map_values(
        |constant| Ok(Array::Constant(Constant::Float(function(constant.alone(1)?.number(0))))),
        |values| {
            let kind = values.kind.floating();
            Ok(Values::float(kind, values.numbers().into_iter().map(|value| kind.round(function(value))).collect()))
        },
    )
}

/// Apply a function that keeps the kind to every value; `what` names it in a refusal.
pub(crate) fn same_map(
    array: &Array,
    what: &str,
    whole: fn(i64) -> i64,
    float: fn(f64) -> f64,
) -> Result<Array> {
    let fixed = |constant: &Constant| match constant {
        Constant::Whole(value) => Ok(Array::Constant(Constant::Whole(whole(*value)))),
        Constant::Float(value) => Ok(Array::Constant(Constant::Float(float(*value)))),
        Constant::Missing(key) => Err(lacking(key, &format!("a number in {what}"))),
        Constant::Truth(_) => Ok(array.clone()),
        Constant::Text(text) => Err(problem(format!("{what} needs a number, not the text {text:?}"))),
    };
    array.map_values(fixed, |values| {
        Ok(match &values.store {
            Store::Whole(held) => Values::whole(values.kind, held.iter().map(|v| values.kind.wrap(whole(*v))).collect()),
            Store::Float(held) => Values::float(values.kind, held.iter().map(|v| float(*v)).collect()),
        })
    })
}

/// Every value rounded to the nearest whole number, halves to even, in its own kind: whole numbers
/// are whole already. True-or-false values are refused: NumPy makes them 16-bit floats to round.
pub(crate) fn rounded(array: &Array) -> Result<Array> {
    let truth = match array {
        Array::Event(values) | Array::Object { values, .. } => values.kind == Kind::B1,
        Array::Constant(constant) => matches!(constant, Constant::Truth(_)),
    };
    if truth {
        return Err(problem("round rounds numbers, not true-or-false values: leave round out for a true-or-false one, or round a number"));
    }
    same_map(array, "round", |v| v, libm::rint)
}

/// Say for every value whether it is a finite number.
pub(crate) fn finite(array: &Array) -> Result<Array> {
    array.map_values(
        |constant| Ok(Array::Constant(Constant::Truth(constant.alone(1)?.number(0).is_finite()))),
        |values| Ok(Values::whole(Kind::B1, values.numbers().into_iter().map(|v| v.is_finite() as i64).collect())),
    )
}

/// The lesser or greater of two arrays value by value, in their common kind, as NumPy's `minimum`
/// and `maximum`: a NaN on either side (the first when both), else the left one only when it is
/// strictly less (greater), so that between equal values the right one is given.
pub(crate) fn pairwise(left: &Array, right: &Array, greatest: bool) -> Result<Array> {
    let float = move |x: f64, y: f64| {
        if x.is_nan() || y.is_nan() {
            if x.is_nan() { x } else { y }
        } else if (greatest && x > y) || (!greatest && x < y) {
            x
        } else {
            y
        }
    };
    let whole = move |kind: Kind, x: i64, y: i64| {
        let (a, b) = (kind.widen(x), kind.widen(y));
        if (greatest && a > b) || (!greatest && a < b) { x } else { y }
    };
    let apply = |a: &Values, b: &Values| -> Result<Values> {
        let (a, b) = together(a, b)?;
        Ok(match (&a.store, &b.store) {
            (Store::Float(x), Store::Float(y)) => Values::float(a.kind, x.iter().zip(y.iter()).map(|(x, y)| float(*x, *y)).collect()),
            (Store::Whole(x), Store::Whole(y)) => Values::whole(a.kind, x.iter().zip(y.iter()).map(|(x, y)| whole(a.kind, *x, *y)).collect()),
            _ => unreachable!("one kind"),
        })
    };
    left.zip(right, &adopt, &|a, b| Ok(apply(&a.alone(1)?, &b.alone(1)?)?.constant()), &apply)
}

/// A float function of two arrays (hypot, atan2), value by value, in their common float kind.
pub(crate) fn float_pair(left: &Array, right: &Array, function: fn(f64, f64) -> f64) -> Result<Array> {
    let apply = |a: &Values, b: &Values| -> Result<Values> {
        let (a, b) = together(a, b)?;
        let kind = a.kind.floating();
        let values = a
            .numbers()
            .into_iter()
            .zip(b.numbers())
            .map(|(x, y)| kind.round(function(x, y)));
        Ok(Values::float(kind, values.collect()))
    };
    left.zip(
        right,
        &adopt,
        &|a, b| {
            Ok(Constant::Float(
                function(a.alone(1)?.number(0), b.alone(1)?.number(0)),
            ))
        },
        &apply,
    )
}

/// The size of each value without its sign; a kind without a sign, `b1` too, is unchanged.
pub(crate) fn magnitude(array: &Array) -> Result<Array> {
    if matches!(array, Array::Event(values) | Array::Object { values, .. } if values.kind.unsigned()) {
        return Ok(array.clone());
    }
    same_map(array, "abs", i64::wrapping_abs, f64::abs)
}

/// The sign of each value: -1, 0, or 1, in the value's kind; for a float ±1, +0 for either zero,
/// and NaN for NaN, as NumPy's `sign`.
pub(crate) fn sign(array: &Array) -> Result<Array> {
    let float = |value: f64| match value {
        _ if value.is_nan() => value,
        _ if value == 0.0 => 0.0,
        _ => value.signum(),
    };
    if matches!(array, Array::Event(values) | Array::Object { values, .. } if values.kind.unsigned()) {
        return same_map(array, "sign", |value| (value != 0) as i64, float);
    }
    same_map(array, "sign", i64::signum, float)
}

/// Each value negated: a float's sign turned over, a whole number subtracted from 0 in its kind
/// (wrapping, as an unsigned kind does); a true-or-false value is refused.
pub(crate) fn negated(array: &Array) -> Result<Array> {
    let truth = || problem("- needs a number, not a true-or-false value: write if(x, -1, 0), or not x");
    let fixed = |constant: &Constant| match constant {
        Constant::Whole(value) => Ok(Array::Constant(Constant::Whole(value.wrapping_neg()))),
        Constant::Float(value) => Ok(Array::Constant(Constant::Float(-value))),
        Constant::Truth(_) => Err(truth()),
        Constant::Missing(key) => Err(lacking(key, "a number")),
        Constant::Text(text) => Err(problem(format!("- needs a number, not the text {text:?}"))),
    };
    array.map_values(fixed, |values| match &values.store {
        Store::Float(held) => Ok(Values::float(values.kind, held.iter().map(|v| -v).collect())),
        Store::Whole(_) if values.kind == Kind::B1 => Err(truth()),
        Store::Whole(held) => Ok(Values::whole(values.kind, held.iter().map(|v| values.kind.wrap(v.wrapping_neg())).collect())),
    })
}

/// Each truth value turned over, for `not`: it takes only true-or-false values.
pub(crate) fn denied(array: &Array) -> Result<Array> {
    let fixed = |constant: &Constant| match constant {
        Constant::Truth(truth) => Ok(Array::Constant(Constant::Truth(!truth))),
        Constant::Missing(key) => Err(lacking(key, "a true-or-false value")),
        Constant::Text(text) => Err(problem(format!("not needs a true-or-false value, not the text {text:?}"))),
        _ => Err(not_truth("not", "a number")),
    };
    array.map_values(fixed, |values| Ok(Values::whole(Kind::B1, values.truths("not")?.iter().map(|v| 1 - v).collect())))
}

/// How many members each event's list has, or for true-or-false members how many are true.
pub(crate) fn count(array: &Array) -> Result<Array> {
    let (offsets, values) = lists(array, "count")?;
    let counts = offsets
        .windows(2)
        .map(|pair| match (&values.store, values.kind) {
            (Store::Whole(held), Kind::B1) => held[pair[0]..pair[1]].iter().sum(),
            _ => (pair[1] - pair[0]) as i64,
        });
    Ok(Array::Event(Values::whole(Kind::I8, counts.collect())))
}

/// The sum of each event's list: floats in their kind, whole numbers in 64 bits.
pub(crate) fn sum(array: &Array) -> Result<Array> {
    let (offsets, values) = lists(array, "sum")?;
    let spans = offsets.windows(2);
    Ok(Array::Event(match &values.store {
        Store::Float(held) => {
            let add = |total: f64, value: &f64| values.kind.round(total + value);
            Values::float(
                values.kind,
                spans
                    .map(|pair| held[pair[0]..pair[1]].iter().fold(0.0, add))
                    .collect(),
            )
        }
        Store::Whole(held) => {
            let kind = if values.kind.unsigned() && values.kind != Kind::B1 {
                Kind::U8
            } else {
                Kind::I8
            };
            let add = |total: i64, value: &i64| total.wrapping_add(*value);
            Values::whole(
                kind,
                spans
                    .map(|pair| held[pair[0]..pair[1]].iter().fold(0, add))
                    .collect(),
            )
        }
    }))
}

/// Whether any, or every, member of each event's list is true.
pub(crate) fn any_all(array: &Array, every: bool) -> Result<Array> {
    let (offsets, values) = lists(array, if every { "all" } else { "any" })?;
    let truths = values.truths(if every { "all" } else { "any" })?;
    let test = |pair: &[usize]| {
        let span = &truths[pair[0]..pair[1]];
        (if every {
            span.iter().all(|v| *v != 0)
        } else {
            span.iter().any(|v| *v != 0)
        }) as i64
    };
    Ok(Array::Event(Values::whole(
        Kind::B1,
        offsets.windows(2).map(test).collect(),
    )))
}

/// Run a list kernel on a column's numbers as they are held: floats as floats, and whole
/// numbers as whole numbers, unsigned 64-bit ones as unsigned, so that none is rounded.
macro_rules! in_own_numbers {
    ($values:expr, |$held:ident| $kernel:expr) => {
        match &$values.store {
            Store::Float($held) => $kernel,
            Store::Whole(bits) if $values.kind == Kind::U8 => {
                let unsigned: Vec<u64> = bits.iter().map(|value| *value as u64).collect();
                let $held = &unsigned;
                $kernel
            }
            Store::Whole($held) => $kernel,
        }
    };
}

/// Values at chosen positions, or `otherwise` where there is no choice, in the kind the two
/// share.
fn filled(
    values: &Values,
    picks: &[Option<usize>],
    otherwise: &Array,
    what: &str,
) -> Result<Values> {
    let Array::Constant(otherwise) = otherwise else {
        return Err(problem(format!(
            "{what} needs a fixed value to use when there is none"
        )));
    };
    let kind = otherwise.beside(values.kind)?.with(values.kind);
    let fallback = otherwise.repeat(kind, 1)?;
    let wide = values.convert(kind)?;
    Ok(match (&wide.store, &fallback.store) {
        (Store::Float(held), Store::Float(other)) => Values::float(
            kind,
            picks
                .iter()
                .map(|pick| pick.map_or(other[0], |at| held[at]))
                .collect(),
        ),
        (Store::Whole(held), Store::Whole(other)) => Values::whole(
            kind,
            picks
                .iter()
                .map(|pick| pick.map_or(other[0], |at| held[at]))
                .collect(),
        ),
        _ => unreachable!("one kind"),
    })
}

/// Positions counting from 0 as a whole-number column, with `otherwise` where there is none.
fn positions_or(picks: &[Option<usize>], otherwise: &Array, what: &str) -> Result<Values> {
    let fallback = match otherwise {
        Array::Constant(Constant::Whole(fallback)) => fallback,
        Array::Constant(Constant::Missing(key)) => return Err(lacking(key, &format!("the whole number {what} uses when there is no member"))),
        _ => return Err(problem(format!("{what} needs a fixed whole number, the same for every event, to use when there is no member"))),
    };
    let positions = picks
        .iter()
        .map(|pick| pick.map_or(*fallback, |at| at as i64));
    Ok(Values::whole(Kind::I8, positions.collect()))
}

/// Computed floats rounded once to `kind`, with `otherwise` where there is none.
fn floats_or(found: &[Option<f64>], kind: Kind, otherwise: &Array, what: &str) -> Result<Values> {
    let Array::Constant(otherwise) = otherwise else {
        return Err(problem(format!(
            "{what} needs a fixed value to use when there is none"
        )));
    };
    let kind = otherwise.beside(kind)?.with(kind);
    let fallback = otherwise.repeat(kind, 1)?.number(0);
    let values = found
        .iter()
        .map(|value| value.map_or(fallback, |v| kind.round(v)));
    Ok(Values::float(kind, values.collect()))
}

/// The float kind a kernel's result takes: the kind the inputs combine in, as a float.
fn float_kind(arrays: &[&Values]) -> Kind {
    arrays.iter().map(|values| values.kind).reduce(Kind::with).map_or(Kind::F8, Kind::floating)
}

/// The least or greatest member of each event's list, or `otherwise` where it is empty: the first
/// one strictly less (greater) than every member before it, as `least_index` finds it.
pub(crate) fn extreme(array: &Array, otherwise: &Array, greatest: bool) -> Result<Array> {
    let what = if greatest { "greatest" } else { "least" };
    let (offsets, values) = lists(array, what)?;
    let places = in_own_numbers!(values, |held| hep_columns::lists::extreme_at(offsets, held, greatest));
    let picks: Vec<Option<usize>> = places.iter().zip(offsets.iter()).map(|(at, start)| at.map(|at| start + at)).collect();
    Ok(Array::Event(filled(values, &picks, otherwise, what)?))
}

/// The member at a position of each event's list, counting from 0, or `otherwise`; `what` names it.
pub(crate) fn nth(array: &Array, position: usize, otherwise: &Array, what: &str) -> Result<Array> {
    let (offsets, values) = lists(array, what)?;
    let picks: Vec<Option<usize>> = offsets.windows(2).map(|pair| (pair[0] + position < pair[1]).then_some(pair[0] + position)).collect();
    Ok(Array::Event(filled(values, &picks, otherwise, what)?))
}

/// Keep the members of each event's list that a true-or-false list of the same shape marks.
pub(crate) fn pick(array: &Array, mask: &Array) -> Result<Array> {
    let (offsets, values) = lists(array, "A pick")?;
    let (others, marks) = lists(mask, "A pick's mask")?;
    let marks = marks.truths("A mask")?;
    if offsets != others {
        return Err(unlike(others, offsets, "A mask has one true-or-false value per member of the list it picks from"));
    }
    let positions: Vec<usize> = (0..values.len()).filter(|at| marks[*at] != 0).collect();
    let counts: Vec<usize> = offsets.windows(2).map(|pair| marks[pair[0]..pair[1]].iter().filter(|mark| **mark != 0).count()).collect();
    Array::object(&counts, values.take(&positions))
}

/// Each member's position in its event's list, counting from 0.
pub(crate) fn index(array: &Array) -> Result<Array> {
    let (offsets, _) = lists(array, "index")?;
    let positions = offsets
        .windows(2)
        .flat_map(|pair| 0..(pair[1] - pair[0]) as i64);
    Ok(Array::Object {
        offsets: offsets.clone(),
        values: Values::whole(Kind::I8, positions.collect()),
    })
}

/// For each member of a list, whether its position appears in the event's list of positions.
pub(crate) fn listed(array: &Array, positions: &Array) -> Result<Array> {
    let (offsets, _) = lists(array, "listed")?;
    let (others, named) = lists(positions, "listed's positions")?;
    if offsets.len() != others.len() {
        return Err(problem("listed needs two lists over the same events"));
    }
    let wanted = whole_positions(named, "listed")?;
    let mut marks = Vec::with_capacity(offsets[offsets.len() - 1]);
    for event in 0..offsets.len() - 1 {
        let here: Vec<i128> = (others[event]..others[event + 1]).map(|at| named.kind.widen(wanted[at])).collect();
        marks.extend((0..offsets[event + 1] - offsets[event]).map(|at| here.contains(&(at as i128)) as i64));
    }
    Ok(Array::Object { offsets: offsets.clone(), values: Values::whole(Kind::B1, marks) })
}

/// The whole numbers of a list of positions, or why it holds none: `what` names the function.
fn whole_positions<'a>(positions: &'a Values, what: &str) -> Result<&'a [i64]> {
    match (&positions.store, positions.kind) {
        (_, Kind::B1) => Err(problem(format!("{what} needs positions, not true-or-false values; use values[mask] to keep members"))),
        (Store::Whole(held), _) => Ok(held),
        (Store::Float(_), kind) => Err(problem(format!("{what} needs whole-number positions, counting from 0, not {}", kind.name()))),
    }
}

/// One list per event, as its offsets and values.
type List<'a> = (&'a Arc<Vec<usize>>, &'a Values);

/// The eta and phi of two lists over the same events, for the operations between them.
fn two_lists<'a>(arguments: &'a [Array], what: &str) -> Result<[List<'a>; 4]> {
    let (offsets, eta) = lists(&arguments[0], what)?;
    let (phi_offsets, phi) = lists(&arguments[1], what)?;
    let (others, other_eta) = lists(&arguments[2], what)?;
    let (other_phi_offsets, other_phi) = lists(&arguments[3], what)?;
    if offsets.len() != others.len() {
        return Err(problem(format!("{what} needs eta and phi of two lists over the same events")));
    }
    if offsets != phi_offsets {
        return Err(unlike(offsets, phi_offsets, &format!("{what} needs the eta and phi of one list, member by member")));
    }
    if others != other_phi_offsets {
        return Err(unlike(others, other_phi_offsets, &format!("{what} needs the eta and phi of the other list, member by member")));
    }
    Ok([
        (offsets, eta),
        (phi_offsets, phi),
        (others, other_eta),
        (other_phi_offsets, other_phi),
    ])
}

/// For each member of one list, its delta R to the `rank`-th nearest (or farthest) member of
/// another list of the same event, or with `position` that member's place in its list; a
/// fifth argument is used where there is no such member, otherwise infinity for a distance
/// and -1 for a position.
pub(crate) fn ranked(
    arguments: &[Array],
    rank: usize,
    farthest: bool,
    position: bool,
    what: &str,
) -> Result<Array> {
    let [(offsets, eta), (_, phi), (others, other_eta), (_, other_phi)] =
        two_lists(arguments, what)?;
    let found = hep_columns::tables::ranked_by_distance(
        offsets,
        &eta.numbers(),
        &phi.numbers(),
        others,
        &other_eta.numbers(),
        &other_phi.numbers(),
        rank,
        farthest,
    );
    let values = if position {
        let picks: Vec<Option<usize>> = found.iter().map(|f| f.map(|(at, _)| at)).collect();
        let otherwise = Array::Constant(Constant::Whole(-1));
        positions_or(&picks, arguments.get(4).unwrap_or(&otherwise), what)?
    } else {
        let kind = float_kind(&[eta, phi, other_eta, other_phi]);
        let distances: Vec<Option<f64>> = found.iter().map(|f| f.map(|(_, d)| d)).collect();
        let otherwise = Array::Constant(Constant::Float(f64::INFINITY));
        floats_or(
            &distances,
            kind,
            arguments.get(4).unwrap_or(&otherwise),
            what,
        )?
    };
    Ok(Array::Object {
        offsets: offsets.clone(),
        values,
    })
}

/// The coordinates of one list of particles: pt, eta, phi, and mass when given.
fn particles<'a>(
    arguments: &'a [Array],
    what: &str,
) -> Result<(&'a Arc<Vec<usize>>, Vec<&'a Values>)> {
    let (offsets, pt) = lists(&arguments[0], what)?;
    let mut held = vec![pt];
    for argument in &arguments[1..] {
        let (own, values) = lists(argument, what)?;
        if own != offsets {
            return Err(unlike(offsets, own, &format!("{what} needs pt, eta, phi, and any mass of the same list, member by member")));
        }
        held.push(values);
    }
    Ok((offsets, held))
}

/// What `of` measures (the mass, pt, eta, phi or rapidity) of each event's list of particles
/// added together, from pt, eta, phi, and a mass column when given; massless without one.
pub(crate) fn summed(arguments: &[Array], name: &str, of: fn(Momentum) -> f64) -> Result<Array> {
    let (offsets, held) = particles(arguments, name)?;
    let kind = float_kind(&held);
    let numbers: Vec<Vec<f64>> = held.iter().map(|values| values.numbers()).collect();
    let mass = numbers.get(3).map(Vec::as_slice);
    let sums = hep_columns::vectors::sum_per_event(offsets, &numbers[0], &numbers[1], &numbers[2], mass);
    Ok(Array::Event(Values::float(kind, sums.map(|sum| kind.round(of(sum))).collect())))
}

/// How many pairs one event's list may make: 65,536 (a list of 362 members makes 65,341).
pub(crate) const MOST_PAIRS: usize = 1 << 16;

/// How many pairs the events worked on at once may make together, past which they are `crowded`.
pub(crate) const RUN_PAIRS: usize = 1 << 26;

/// `RUN_PAIRS`; in a debug build, `RVR_RUN_PAIRS` when set, so the tests can make a run crowded.
fn run_pairs() -> usize {
    #[cfg(debug_assertions)]
    if let Some(n) = std::env::var("RVR_RUN_PAIRS").ok().and_then(|n| n.parse().ok()).filter(|n| *n > 0) {
        return n;
    }
    RUN_PAIRS
}

/// The pairs of each event's list as positions into its values; an event whose list would make
/// more than `MOST_PAIRS` is a data error, and events making more than `RUN_PAIRS` together are
/// too many to work on at once.
fn pairs_of(offsets: &[usize], what: &str) -> Result<(hep_columns::Jagged<usize>, Vec<usize>)> {
    let made = |members: usize| members * members.saturating_sub(1) / 2;
    if let Some(event) = (0..offsets.len() - 1).find(|&e| made(offsets[e + 1] - offsets[e]) > MOST_PAIRS) {
        let members = offsets[event + 1] - offsets[event];
        return Err(data(event, format!("{what} would make {} pairs of this event's {members} members, more than the {MOST_PAIRS} one event may make: select fewer members first, as Jet_pt[Jet_pt > 30]", made(members))));
    }
    let count = hep_columns::lists::pair_count(offsets);
    if count > run_pairs() && offsets.len() > 2 {
        #[cfg(debug_assertions)]
        if run_pairs() != RUN_PAIRS {
            static SAID: std::sync::Once = std::sync::Once::new();
            SAID.call_once(|| eprintln!("rvr (a debug build): RVR_RUN_PAIRS: {count} pairs over {} events, more than {}: worked on in halves", offsets.len() - 1, run_pairs()));
        }
        return Err(crate::crowded(format!("{what} would make {count} pairs over these events, more than one run of them may hold at once")));
    }
    Ok(hep_columns::lists::pairs(offsets))
}

/// For each pair of members of each event's list, the first member's value, or the second's.
pub(crate) fn pair_member(array: &Array, second: bool) -> Result<Array> {
    let what = if second { "pair_second" } else { "pair_first" };
    let (offsets, values) = lists(array, what)?;
    let (first, other) = pairs_of(offsets, what)?;
    let positions = if second { &other } else { &first.values };
    Ok(Array::Object {
        offsets: Arc::new(first.offsets),
        values: values.take(positions),
    })
}

/// What `of` measures of each pair of particles in each event's list added together.
pub(crate) fn pair_summed(arguments: &[Array], name: &str, of: fn(Momentum) -> f64) -> Result<Array> {
    let (offsets, held) = particles(arguments, name)?;
    let (first, second) = pairs_of(offsets, name)?;
    let kind = float_kind(&held);
    let numbers: Vec<Vec<f64>> = held.iter().map(|values| values.numbers()).collect();
    let mass = numbers.get(3).map(Vec::as_slice);
    let sums = hep_columns::vectors::pair_sum(&first.values, &second, &numbers[0], &numbers[1], &numbers[2], mass);
    Ok(Array::Object {
        offsets: Arc::new(first.offsets),
        values: Values::float(kind, sums.map(|sum| kind.round(of(sum))).collect()),
    })
}

/// The delta R between the two members of each pair of each event's list.
pub(crate) fn pair_dr(eta: &Array, phi: &Array) -> Result<Array> {
    let (offsets, eta) = lists(eta, "pair_dr")?;
    let (phi_offsets, phi) = lists(phi, "pair_dr")?;
    if offsets != phi_offsets {
        return Err(unlike(offsets, phi_offsets, "pair_dr needs the eta and phi of one list, member by member"));
    }
    let (first, second) = pairs_of(offsets, "pair_dr")?;
    let kind = float_kind(&[eta, phi]);
    let (eta, phi) = (eta.numbers(), phi.numbers());
    let distances = first.values.iter().zip(&second).map(|(a, b)| {
        kind.round(hep_columns::angles::delta_r(
            eta[*a], phi[*a], eta[*b], phi[*b],
        ))
    });
    Ok(Array::Object {
        offsets: Arc::new(first.offsets),
        values: Values::float(kind, distances.collect()),
    })
}

/// The position in each event's list of its least member (or greatest), counting from 0, or
/// `otherwise` where the list is empty.
pub(crate) fn extreme_at(array: &Array, otherwise: &Array, greatest: bool) -> Result<Array> {
    let what = if greatest {
        "greatest_index"
    } else {
        "least_index"
    };
    let (offsets, values) = lists(array, what)?;
    let picks = in_own_numbers!(values, |held| hep_columns::lists::extreme_at(
        offsets, held, greatest
    ));
    Ok(Array::Event(positions_or(&picks, otherwise, what)?))
}

/// Each event's list in ascending order of value (or descending): the members themselves, or
/// with `positions` their positions counting from 0.
pub(crate) fn order(array: &Array, descending: bool, positions: bool) -> Result<Array> {
    let what = if positions { "order" } else { "sorted" };
    let (offsets, values) = lists(array, what)?;
    let ordered = in_own_numbers!(values, |held| hep_columns::lists::order(
        offsets, held, descending
    ));
    let values = if positions {
        let positions = ordered.values.iter().map(|at| *at as i64);
        Values::whole(Kind::I8, positions.collect())
    } else {
        // The positions count from the start of each list; the values are laid end to end.
        let flat: Vec<usize> = offsets
            .windows(2)
            .zip(ordered.offsets.windows(2))
            .flat_map(|(list, own)| {
                ordered.values[own[0]..own[1]]
                    .iter()
                    .map(move |at| list[0] + at)
            })
            .collect();
        values.take(&flat)
    };
    Ok(Array::Object {
        offsets: offsets.clone(),
        values,
    })
}

/// For each event, the members of a list at each of a list of positions, or `otherwise` where
/// a position is negative or past the end of the list.
pub(crate) fn take(array: &Array, positions: &Array, otherwise: &Array) -> Result<Array> {
    let (offsets, values) = lists(array, "take")?;
    let (position_offsets, wanted) = lists(positions, "take's positions")?;
    if offsets.len() != position_offsets.len() {
        return Err(problem("take needs a list and positions over the same events"));
    }
    // A u8 position past the largest i8 is held as a negative number: past the end either way.
    let held = whole_positions(wanted, "take")?;
    let taken = hep_columns::lists::take(offsets, position_offsets, held);
    Ok(Array::Object { offsets: position_offsets.clone(), values: filled(values, &taken.values, otherwise, "take")? })
}

/// For each event, the members of several lists one after another, in the order given, in the
/// kind they combine in.
pub(crate) fn join(arguments: &[Array]) -> Result<Array> {
    let lists = arguments.iter().map(|argument| lists(argument, "join")).collect::<Result<Vec<_>>>()?;
    let kind = lists.iter().map(|(_, values)| values.kind).reduce(Kind::with).ok_or_else(|| problem("join needs two lists or more"))?;
    // Each list in that kind (itself when it is already): floats or whole numbers, as it holds.
    let cast: Vec<Values> = lists.iter().map(|(_, values)| values.convert(kind)).collect::<Result<_>>()?;
    let offsets = lists.iter().map(|(offsets, _)| offsets.as_slice());
    let joined = if kind.float() {
        let parts: Option<Vec<(&[usize], &[f64])>> = offsets.zip(&cast).map(|(o, v)| v.floats().map(|held| (o, held))).collect();
        parts.and_then(|parts| hep_columns::lists::join(&parts)).map(|j| (j.offsets, Values::float(kind, j.values)))
    } else {
        let parts: Option<Vec<(&[usize], &[i64])>> = offsets.zip(&cast).map(|(o, v)| v.wholes().map(|held| (o, held))).collect();
        parts.and_then(|parts| hep_columns::lists::join(&parts)).map(|j| (j.offsets, Values::whole(kind, j.values)))
    };
    let (offsets, values) = joined.ok_or_else(|| problem("join needs lists over the same events"))?;
    Ok(Array::Object {
        offsets: Arc::new(offsets),
        values,
    })
}

/// A formula applied value by value over several arrays: constants and one-per-event values
/// are spread over any list among them, and the result is computed in 64 bits and rounded
/// once to the float kind the arrays combine in.
pub(crate) fn formula(
    arrays: &[&Array],
    what: &str,
    function: impl Fn(&[f64]) -> f64,
) -> Result<Array> {
    let offsets = arrays.iter().find_map(|array| match array {
        Array::Object { offsets, .. } => Some(offsets),
        _ => None,
    });
    let columns: Vec<&Values> = arrays
        .iter()
        .filter_map(|array| match array {
            Array::Event(values) | Array::Object { values, .. } => Some(values),
            Array::Constant(_) => None,
        })
        .collect();
    let mut kind = float_kind(&columns);
    for array in arrays {
        if let Array::Constant(constant) = array {
            kind = constant.beside(kind)?.with(kind);
        }
    }
    let Some(events) = arrays.iter().find_map(|array| array.events()) else {
        let values: Vec<f64> = arrays
            .iter()
            .map(|array| match array {
                Array::Constant(constant) => constant.alone(1).map(|held| held.number(0)),
                _ => unreachable!("no events means every argument is a constant"),
            })
            .collect::<Result<_>>()?;
        return Ok(Array::Constant(Constant::Float(function(&values))));
    };
    let laid: Vec<Vec<f64>> = arrays
        .iter()
        .map(|array| laid(array, Kind::F8, events, offsets, what).map(|values| values.numbers()))
        .collect::<Result<_>>()?;
    let count = laid.first().map_or(0, Vec::len);
    let mut at = vec![0.0; arrays.len()];
    let values = (0..count).map(|index| {
        for (slot, column) in at.iter_mut().zip(&laid) {
            *slot = column[index];
        }
        kind.round(function(&at))
    });
    let values = Values::float(kind, values.collect());
    Ok(match offsets {
        Some(offsets) => Array::Object {
            offsets: offsets.clone(),
            values,
        },
        None => Array::Event(values),
    })
}

/// A bit operation on whole numbers with a fixed second value, value by value, keeping the kind.
pub(crate) fn bits(
    array: &Array,
    amount: &Array,
    what: &str,
    function: impl Fn(i64, i64, bool) -> i64,
) -> Result<Array> {
    let amount = match amount {
        Array::Constant(Constant::Whole(amount)) => amount,
        Array::Constant(Constant::Missing(key)) => return Err(lacking(key, &format!("the second value of {what}"))),
        _ => return Err(problem(format!("{what} needs a fixed whole number, the same for every event, as its second value"))),
    };
    let apply = |values: &Values| match &values.store {
        Store::Whole(_) if values.kind == Kind::B1 => Err(problem(format!(
            "{what} needs whole numbers, not true-or-false values"
        ))),
        Store::Whole(held) => {
            // The second value must be one the column's kind can hold, as NumPy requires.
            Constant::Whole(*amount).beside(values.kind)?;
            Ok(Values::whole(
                values.kind,
                held.iter()
                    .map(|v| {
                        values
                            .kind
                            .wrap(function(*v, *amount, values.kind.unsigned()))
                    })
                    .collect(),
            ))
        }
        Store::Float(_) => Err(problem(format!(
            "{what} needs whole numbers, not {}",
            values.kind.name()
        ))),
    };
    let fixed = |constant: &Constant| match constant {
        Constant::Whole(value) => Ok(Array::Constant(Constant::Whole(function(*value, *amount, false)))),
        Constant::Missing(key) => Err(lacking(key, &format!("a whole number in {what}"))),
        _ => Err(problem(format!("{what} needs whole numbers"))),
    };
    array.map_values(fixed, apply)
}

/// Lay an array out over a shape: per event, or per member when `offsets` is given.
fn laid(
    array: &Array,
    kind: Kind,
    events: usize,
    offsets: Option<&Arc<Vec<usize>>>,
    what: &str,
) -> Result<Values> {
    if array.events().is_some_and(|own| own != events)
        || offsets.is_some_and(|offsets| offsets.len() != events + 1)
    {
        return Err(problem(format!(
            "The parts of {what} do not cover the same events"
        )));
    }
    let members = offsets.map_or(events, |offsets| offsets[events]);
    match (array, offsets) {
        (Array::Constant(constant), _) => {
            constant.beside(kind)?;
            constant.repeat(kind, members)
        }
        (Array::Event(values), None) => values.convert(kind),
        (Array::Event(values), Some(offsets)) => Array::spread(values, offsets).convert(kind),
        (Array::Object { offsets: own, values }, Some(offsets)) if own == offsets => values.convert(kind),
        (Array::Object { offsets: own, .. }, Some(offsets)) => Err(unlike(offsets, own, &format!("The lists of {what} are taken member by member"))),
        _ => Err(problem(format!("The parts of {what} do not cover the same events and members"))),
    }
}

/// Choose, value by value, from `yes` where the condition is true and from `no` elsewhere; the
/// condition is a true-or-false value.
pub(crate) fn choose(condition: &Array, yes: &Array, no: &Array) -> Result<Array> {
    match condition {
        Array::Constant(Constant::Missing(key)) => return Err(lacking(key, "the condition of if")),
        Array::Constant(Constant::Text(text)) => return Err(problem(format!("if's condition needs a true-or-false value, not the text {text:?}"))),
        Array::Constant(_) => return Err(not_truth("if's condition", "a fixed value")),
        Array::Event(values) | Array::Object { values, .. } if values.kind != Kind::B1 => return Err(not_truth("if's condition", values.kind.name())),
        _ => {}
    }
    let parts = [condition, yes, no];
    let offsets = parts.iter().find_map(|part| match part {
        Array::Object { offsets, .. } => Some(offsets),
        _ => None,
    });
    let events = condition.events().unwrap_or(0);
    let kind_of = |array: &Array| match array {
        Array::Constant(_) => None,
        Array::Event(values) | Array::Object { values, .. } => Some(values.kind),
    };
    let kind = match (kind_of(yes), kind_of(no), yes, no) {
        (Some(a), Some(b), ..) => a.with(b),
        (Some(a), None, _, Array::Constant(c)) | (None, Some(a), Array::Constant(c), _) => c.beside(a)?.with(a),
        (None, None, Array::Constant(a), Array::Constant(b)) => together(&a.alone(1)?, &b.alone(1)?)?.0.kind,
        _ => unreachable!("an array without a kind is a constant"),
    };
    let marks = laid(condition, Kind::B1, events, offsets, "an if")?;
    let marks = marks.truths("if")?;
    let (yes, no) = (laid(yes, kind, events, offsets, "an if")?, laid(no, kind, events, offsets, "an if")?);
    let pick = |at: usize| marks[at] != 0;
    let values = match (&yes.store, &no.store) {
        (Store::Float(a), Store::Float(b)) => Values::float(kind, (0..marks.len()).map(|at| if pick(at) { a[at] } else { b[at] }).collect()),
        (Store::Whole(a), Store::Whole(b)) => Values::whole(kind, (0..marks.len()).map(|at| if pick(at) { a[at] } else { b[at] }).collect()),
        _ => unreachable!("one kind"),
    };
    Ok(match offsets {
        Some(offsets) => Array::Object { offsets: offsets.clone(), values },
        None => Array::Event(values),
    })
}
