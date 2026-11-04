//! What a press shows while it reads. On a terminal, one bar per sample (its entries read of those
//! chosen), redrawn at most 5 times a second by a thread of its own from counters the readers add
//! to once per run of entries, with what was said meanwhile (a wait, a hole) printed above the
//! bars; elsewhere (a log, a batch job), one line per file read, as it finishes.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the bars are redrawn, and the terminal's size asked again; how wide a line and a bar
/// may be (a long name takes room from the bar, so every line fits in 80 columns); and how many
/// samples get a bar of their own.
const EVERY: Duration = Duration::from_millis(200);
const RESIZED: u32 = 10;
const LINE: usize = 79;
const WIDE: (usize, usize) = (10, 20);
const SHOWN: usize = 10;

/// What the readers of one press count, for the bars.
pub struct Counts {
    /// Each sample's name, and the entries chosen of it.
    samples: Vec<(String, u64)>,
    /// Each sample's entries read, or found to be in holes, and whether a file of it has begun.
    done: Vec<AtomicU64>,
    begun: Vec<AtomicBool>,
    kept: AtomicU64,
    /// The bytes read from every file, which each file adds to as it reads.
    pub bytes: Arc<AtomicU64>,
    holes: AtomicUsize,
    /// Whether these are drawn as bars.
    drawn: AtomicBool,
}

impl Counts {
    /// `entries` of sample `sample` read (or found in holes), keeping `kept`.
    pub fn ran(&self, sample: usize, entries: u64, kept: u64) {
        self.done[sample].fetch_add(entries, Ordering::Relaxed);
        self.kept.fetch_add(kept, Ordering::Relaxed);
    }

    /// A hole found.
    pub fn holed(&self) {
        self.holes.fetch_add(1, Ordering::Relaxed);
    }

    /// A file of sample `sample` begun.
    pub fn begin(&self, sample: usize) {
        self.begun[sample].store(true, Ordering::Relaxed);
    }
}

/// The bars on the terminal: what they show, since when, how big the terminal is (rows, columns),
/// and how wide each line they took is.
struct Drawn {
    counts: Arc<Counts>,
    started: Instant,
    size: (usize, usize),
    lines: Vec<usize>,
}

/// The bars of the press under way, when there are bars.
static DRAWN: Mutex<Option<Drawn>> = Mutex::new(None);

/// A file read, as one line: said unless these counts are drawn as bars, which show it instead.
pub fn file(counts: &Counts, line: &str) {
    if !counts.drawn.load(Ordering::Relaxed) {
        say(line);
    }
}

/// Print a line above the bars while there are bars, panicking as `println!` does on a closed pipe.
pub fn say(line: &str) {
    let mut drawn = DRAWN.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = std::io::stdout().lock();
    let written = match drawn.as_mut() {
        Some(drawn) => {
            let cleared = clear(drawn);
            write!(out, "{cleared}{line}\n{}", draw(drawn))
        }
        None => writeln!(out, "{line}"),
    };
    if let Err(e) = written.and_then(|()| out.flush()) {
        panic!("failed printing to stdout: {e}");
    }
}

/// What moves up over the lines last drawn (each as many rows as it takes at the terminal's width
/// now) and clears them and what follows.
fn clear(drawn: &Drawn) -> String {
    let columns = drawn.size.1.max(1);
    let rows: usize = drawn.lines.iter().map(|wide| wide.div_ceil(columns).max(1)).sum();
    match rows {
        0 => String::new(),
        rows => format!("\x1b[{rows}A\x1b[J"),
    }
}

/// How wide the names and the bars of these samples (name, chosen) are drawn, to fit in `line`.
fn widths(samples: &[(&str, u64)], line: usize) -> (usize, usize) {
    let numbers = samples.iter().map(|(_, chosen)| 2 * rowver::rvr::grouped(*chosen).len() + 3).max().unwrap_or(0);
    let longest = samples.iter().map(|(name, _)| name.chars().count()).max().unwrap_or(0);
    let room = line.saturating_sub(2 + 2 + 1 + 4 + 2 + numbers);
    let name = longest.min(room.saturating_sub(WIDE.0));
    (name, room.saturating_sub(name).clamp(WIDE.0, WIDE.1))
}

/// `done` of `chosen` as a bar `wide` across and a percent: full, and 100, only when all are done.
pub fn cells(done: u64, chosen: u64, wide: usize) -> (String, f64) {
    let done = done.min(chosen);
    let part = if chosen == 0 { 1.0 } else { done as f64 / chosen as f64 };
    let full = ((part * wide as f64).floor() as usize).min(wide);
    let full = if done < chosen { full.min(wide - 1) } else { full };
    let percent = if done < chosen { (part * 100.0).floor().min(99.0) } else { 100.0 };
    (format!("{}{}", "█".repeat(full), "░".repeat(wide - full)), percent)
}

/// One sample's bar, `wide` across: its name (in `width`), the percent, its entries done of chosen.
fn bar(name: &str, done: u64, chosen: u64, (width, wide): (usize, usize)) -> String {
    let (cells, percent) = cells(done, chosen, wide);
    let name: String = match name.chars().count() > width {
        true => name.chars().take(width.saturating_sub(1)).chain(['…']).collect(),
        false => name.to_string(),
    };
    let shown = format!("{} / {}", rowver::rvr::grouped(done.min(chosen)), rowver::rvr::grouped(chosen));
    format!("  {name:width$}  {cells} {percent:>3.0}%  {shown}")
}

/// The bars and the line under them, each cut to the terminal's width, as many bars as fit in its
/// height; noted in `drawn` (each line's width), to be cleared before the next.
fn draw(drawn: &mut Drawn) -> String {
    let frame = frame(drawn);
    drawn.lines = frame.lines().map(|line| line.chars().count()).collect();
    frame
}

/// The text of the bars and the line under them.
fn frame(drawn: &Drawn) -> String {
    let counts = &drawn.counts;
    let seconds = drawn.started.elapsed().as_secs_f64();
    let (rows, columns) = drawn.size;
    let line = LINE.min(columns.saturating_sub(1));
    let fits = |text: String| -> String { text.chars().take(line).collect() };
    // A sample this press reads nothing of has no bar.
    let samples: Vec<(&str, u64, u64, bool)> = counts
        .samples
        .iter()
        .zip(&counts.done)
        .zip(&counts.begun)
        .filter(|(((_, chosen), _), _)| *chosen > 0)
        .map(|(((name, chosen), done), begun)| {
            let done = done.load(Ordering::Relaxed).min(*chosen);
            (name.as_str(), done, *chosen, begun.load(Ordering::Relaxed) || done > 0)
        })
        .collect();
    // With more samples than fit, those under way get a bar, then the next to come, and the rest
    // a line; the widths are of every sample, so they hold as samples come and go.
    let room = SHOWN.min(rows.saturating_sub(5)).max(1);
    let under_way = |(_, done, chosen, begun): &&(&str, u64, u64, bool)| *begun && done < chosen;
    let to_come = |(_, _, _, begun): &&(&str, u64, u64, bool)| !*begun;
    let shown: Vec<&(&str, u64, u64, bool)> = match samples.len() > room {
        true => samples.iter().filter(under_way).chain(samples.iter().filter(to_come)).take(room - 1).collect(),
        false => samples.iter().collect(),
    };
    let fit = widths(&samples.iter().map(|(name, _, chosen, _)| (*name, *chosen)).collect::<Vec<_>>(), line);
    let mut text = String::from("\n");
    for (name, done, chosen, _) in &shown {
        text += &format!("{}\n", fits(bar(name, *done, *chosen, fit)));
    }
    if samples.len() > shown.len() {
        let rest: Vec<_> = samples.iter().filter(|sample| !shown.iter().any(|shown| std::ptr::eq(*shown, *sample))).collect();
        let waiting = rest.iter().copied().filter(to_come).count();
        let going = rest.iter().copied().filter(under_way).count();
        let more = rest.len();
        text += &format!("{}\n", fits(format!("  and {more} more: {} done, {waiting} to come, {going} under way", more - waiting - going)));
    }
    let bytes = counts.bytes.load(Ordering::Relaxed) as f64;
    let rate = match seconds > 1.0 {
        true => format!(" · {:.0} MB/s", bytes / 1e6 / seconds),
        false => String::new(),
    };
    let holes = match counts.holes.load(Ordering::Relaxed) {
        0 => String::new(),
        n => format!(" · {}", crate::samples::some(n, "hole", "holes")),
    };
    let kept = rowver::rvr::grouped(counts.kept.load(Ordering::Relaxed));
    text + &format!("\n{}\n", fits(format!("  {kept} kept so far{holes}{rate} · {}", elapsed(seconds))))
}

/// How big the terminal is, as (rows, columns): what `stty` says, or `LINES` and `COLUMNS`, or 24 by 80.
fn size() -> (usize, usize) {
    let asked = || -> Option<(usize, usize)> {
        let tty = std::fs::File::open("/dev/tty").ok()?;
        let said = std::process::Command::new("stty").arg("size").stdin(tty).output().ok()?;
        let said = String::from_utf8(said.stdout).ok()?;
        let mut size = said.split_whitespace().map(|n| n.parse::<usize>().ok().filter(|n| *n > 0));
        Some((size.next()??, size.next()??))
    };
    let given = |name: &str, or| std::env::var(name).ok().and_then(|n| n.parse::<usize>().ok()).filter(|n| *n > 0).unwrap_or(or);
    asked().unwrap_or_else(|| (given("LINES", 24), given("COLUMNS", 80)))
}

/// A time in words: `14.2 s`, `2 min 14 s`.
pub fn elapsed(seconds: f64) -> String {
    match seconds.round() as u64 {
        0..60 => format!("{seconds:.1} s"),
        whole => format!("{} min {} s", whole / 60, whole % 60),
    }
}

/// The display of one press: bars on a terminal, redrawn until it is dropped.
pub struct Display {
    /// What its readers count.
    pub counts: Arc<Counts>,
    /// The drawer, and what wakes it to stop.
    drawer: Option<(std::thread::JoinHandle<()>, Sender<()>)>,
}

impl Display {
    /// Show bars on a terminal for these samples (name, entries chosen, entries done), `kept` and `holes` so far.
    pub fn start(samples: Vec<(String, u64, u64)>, kept: u64, holes: usize) -> Self {
        let counts = Arc::new(Counts {
            done: samples.iter().map(|(_, _, done)| AtomicU64::new(*done)).collect(),
            begun: samples.iter().map(|_| AtomicBool::new(false)).collect(),
            samples: samples.into_iter().map(|(name, chosen, _)| (name, chosen)).collect(),
            kept: AtomicU64::new(kept),
            bytes: Default::default(),
            holes: AtomicUsize::new(holes),
            drawn: AtomicBool::new(false),
        });
        // Bars on a terminal that draws, one press's at a time, not while requests are traced
        // below them, and only when the terminal has room for them.
        let quiet = std::env::var_os("RVR_TRACE").is_none() && std::env::var("TERM").map_or(true, |t| t != "dumb");
        let mut drawn = DRAWN.lock().unwrap_or_else(|e| e.into_inner());
        let size = (std::io::stdout().is_terminal() && quiet && drawn.is_none()).then(size);
        let drawer = size.filter(|(rows, columns)| *rows > 6 && *columns > 40).map(|size| {
            counts.drawn.store(true, Ordering::Relaxed);
            *drawn = Some(Drawn { counts: counts.clone(), started: Instant::now(), size, lines: Vec::new() });
            let (stop, stopped) = std::sync::mpsc::channel::<()>();
            let drawer = std::thread::spawn(move || {
                for frame in 1.. {
                    redraw((frame % RESIZED == 0).then(self::size));
                    if !matches!(stopped.recv_timeout(EVERY), Err(RecvTimeoutError::Timeout)) {
                        break;
                    }
                }
            });
            (drawer, stop)
        });
        Display { counts, drawer }
    }
}

/// Draw the bars again in place, in one write, at the terminal's new `size` when it was asked
/// again (every `RESIZED` times, outside the lock), so a window resized is cleared and drawn to it.
fn redraw(size: Option<(usize, usize)>) {
    let mut drawn = DRAWN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(drawn) = drawn.as_mut() {
        if let Some(size) = size {
            drawn.size = size;
        }
        let frame = clear(drawn) + &draw(drawn);
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(frame.as_bytes()).and_then(|()| out.flush());
    }
}

impl Drop for Display {
    /// The bars drawn a last time, as they end, and left on the terminal.
    fn drop(&mut self) {
        if let Some((drawer, stop)) = self.drawer.take() {
            drop(stop);
            let _ = drawer.join();
            redraw(None);
            *DRAWN.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bar_is_full_only_when_its_sample_is_read_and_every_line_fits_80_columns() {
        let fit = widths(&[("DY", 100_000)], LINE);
        assert_eq!(fit, (2, 20));
        assert_eq!(bar("DY", 0, 0, fit), format!("  DY  {} 100%  0 / 0", "█".repeat(20)));
        let most = bar("DY", 99_999, 100_000, fit);
        assert!(most.contains("99%") && most.contains('░'), "{most}");
        // A long name takes room from the bar, and only one too long for the line is cut.
        let named = "DYJetsToLL_M-50_amcatnloFXFX";
        let fit = widths(&[(named, 195_510_810)], LINE);
        let long = bar(named, 195_510_810, 195_510_810, fit);
        assert!(long.contains(named) && long.contains("100%") && fit.1 < WIDE.1, "{long}");
        assert!(long.chars().count() <= LINE, "{} columns: {long}", long.chars().count());
        let longer = "TTTo2L2Nu_TuneCP5_13TeV-powheg-pythia8";
        let fit = widths(&[(longer, 195_510_810)], LINE);
        let cut = bar(longer, 1, 195_510_810, fit);
        assert!(cut.contains('…') && cut.chars().count() <= LINE && fit.1 == WIDE.0, "{cut}");
        assert_eq!((elapsed(14.24), elapsed(134.0)), ("14.2 s".to_string(), "2 min 14 s".to_string()));
    }

    #[test]
    fn bars_fit_the_terminal_and_leave_out_what_the_press_does_not_read() {
        let samples: Vec<(String, u64)> = (0..13).map(|i| (format!("sample_{i}"), if i == 0 { 0 } else { 1_000 })).collect();
        let done = [0, 1_000, 1_000, 500, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let counts = Arc::new(Counts {
            done: done.iter().map(|d| AtomicU64::new(*d)).collect(),
            begun: done.iter().map(|_| AtomicBool::new(false)).collect(),
            samples,
            kept: AtomicU64::new(7),
            bytes: Default::default(),
            holes: AtomicUsize::new(1),
            drawn: AtomicBool::new(true),
        });
        let drawn = |size| {
            let mut drawn = Drawn { counts: counts.clone(), started: Instant::now(), size, lines: Vec::new() };
            let text = draw(&mut drawn);
            (text, drawn.lines.len())
        };
        let (text, lines) = drawn((50, 120));
        assert_eq!(text.lines().count(), lines, "{text}");
        // sample_0 reads nothing: no bar. Of the other 12, the one under way gets a bar, then the
        // next to come.
        assert!(!text.contains("sample_0 ") && text.contains("sample_3") && text.contains("sample_11"), "{text}");
        assert!(!text.contains("sample_12"), "{text}");
        assert!(text.contains("and 3 more: 2 done, 1 to come, 0 under way"), "{text}");
        assert!(text.contains("7 kept so far · 1 hole"), "{text}");
        // A narrow terminal: no line wider; a short one: no more lines than it has rows.
        let (narrow, lines) = drawn((50, 40));
        assert_eq!(narrow.lines().count(), lines, "{narrow}");
        assert!(narrow.lines().all(|line| line.chars().count() < 40), "{narrow}");
        let (short, lines) = drawn((8, 120));
        assert!(lines < 8 && short.lines().count() == lines, "{short}");
    }
}
