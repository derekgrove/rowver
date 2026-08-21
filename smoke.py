"""Boot a built page in JavaScriptCore against a DOM stub, with its real data."""
import base64, gzip, json, pathlib, re, subprocess, sys

STUB = r"""
var LOG = [];
var LAY = null;
// `plot<n>` is deliberately NOT here: those divs are made by renderPlots, so letting the
// stub conjure them would hide exactly the bug where one is used before it exists.
var DYN = /^(vv|lo|hi|nlo|nhi|fl|ch)[0-9]*$/;
// <details> ids the MARKUP ships open. Without this every El reported open=false whatever
// the page said, so "every section ships closed" passed on markup that shipped them open.
var _OPEN = (typeof OPENIDS !== "undefined") ? OPENIDS : [];
function El(id){ this.id=id; this.style={}; this.dataset={}; this.value=""; this.checked=false;
  this.open = _OPEN.indexOf(id) >= 0;
  this.textContent=""; this.hidden=false; this.options=[]; this._html="";
  this.classList={add:function(){},remove:function(){},toggle:function(){}}; }
// Assigning innerHTML creates elements in a browser; the stub must do the same, or a
// div used before it is built looks fine here and throws in the page.
Object.defineProperty(El.prototype, "innerHTML", {
  get:function(){ return this._html; },
  set:function(v){ this._html = String(v);
    var re = /id="([A-Za-z_][A-Za-z0-9_-]*)"/g, m;
    while ((m = re.exec(this._html)) !== null) if (!DOC[m[1]]) DOC[m[1]] = new El(m[1]);
  }});
El.prototype.querySelectorAll=function(){ return []; };
El.prototype.querySelector=function(){ return new El("q"); };
El.prototype.on=function(ev, fn){ (this._h = this._h || {})[ev] = fn; };  // plotly
  // attaches this to a graph div; keeping the callback lets a test fire the event
El.prototype.setAttribute=function(){}; El.prototype.getAttribute=function(){ return null; };
El.prototype.removeAttribute=function(){};
El.prototype.appendChild=function(){}; El.prototype.remove=function(){};
El.prototype.addEventListener=function(){}; El.prototype.scrollIntoView=function(){};
El.prototype.select=function(){};
var DOC = {};
IDS.forEach(function(i){ DOC[i] = new El(i); });
var document = {
  getElementById:function(id){
    if (DOC[id]) return DOC[id];
    if (DYN.test(id)) { DOC[id] = new El(id); return DOC[id]; }
    // A plot div legitimately does not exist before renderPlots builds it, and the page
    // guards for that. Plotly.react/purge below are what turn a real miss into a throw.
    if (!/^plot[0-9]+$/.test(id)) LOG.push(id);
    return null; },
  querySelectorAll:function(){ return []; }, querySelector:function(){ return new El("q"); },
  createElement:function(){ return new El("x"); }, execCommand:function(){},
  __make:function(id){ DOC[id] = new El(id); },
  body:new El("body"), documentElement:new El("html"), addEventListener:function(){} };
var window = {};
var location = {pathname:"/Users/x/RowVer/studies/s/s.html"};
// a DISTINCT value per custom property. Returning one colour for all of them made any
// "these two are the same colour" check pass on everything, which is no check at all.
function getComputedStyle(){ return { getPropertyValue:function(p){
  return "#" + String(p).replace(/[^a-z0-9]/gi, "").slice(-6); } }; }
var DRAWN = [];                       // so trace names can be asserted, not just eyeballed
var Plotly = {
  react:function(id, traces, lay){ if(!DOC[id]) throw new Error("No DOM element with id '"+id+"' exists on the page.");
    DRAWN = traces || []; LAY = lay || null; },
  purge:function(id){ if(!DOC[id]) throw new Error("No DOM element with id '"+id+"' exists on the page."); },
  downloadImage:function(){},
  toImage:function(){ return Promise.resolve("data:image/png;base64,"); } };
var URL = { createObjectURL:function(){ return "blob:x"; }, revokeObjectURL:function(){} };
// say() defers a call to clear its message; without this any code path that reports
// something throws ReferenceError instead of being tested.
function setTimeout(){ return 0; }
function clearTimeout(){}
// Keeps its parts, so the .rvr writer's own bytes can be inspected without a browser.
function Blob(parts){ this.parts = parts || []; }
function TextEncoder(){}
TextEncoder.prototype.encode = function(s){
  var out = [];
  for (var i = 0; i < s.length; i++){
    var c = s.charCodeAt(i);
    if (c < 0x80) out.push(c);
    else if (c < 0x800) out.push(0xC0|c>>6, 0x80|c&63);
    else out.push(0xE0|c>>12, 0x80|c>>6&63, 0x80|c&63);
  }
  return new Uint8Array(out);
};
"""

def run(html: str, rowver: str | None = None, probe: str = "") -> str:
    """Boot a page (optionally over a dropped .rvr), with `probe` as an extra final check."""
    h = pathlib.Path(html).read_text()
    ids = sorted(set(re.findall(r'id="([A-Za-z_][\w-]*)"', h)))
    # which <details> ship open, so the stub can tell a closed default from an open one
    open_ids = []
    for tag in re.findall(r"<details([^>]*)>", h):
        got = re.search(r'id="([A-Za-z_][\w-]*)"', tag)
        if got and re.search(r"\bopen\b", tag):
            open_ids.append(got.group(1))
    src = re.findall(r"<script>(.*?)</script>", h, re.S)[-1]
    if rowver:
        # what a drop does: the file's META replaces the page's, columns and view together
        blob = gzip.decompress(pathlib.Path(rowver).read_bytes())
        length = int.from_bytes(blob[:4], "little")
        meta, raw = json.loads(blob[4:4 + length]), list(blob[4 + length:])
    else:
        raw_b64 = re.search(r"^let B64   = (.*);$", h, re.M).group(1)
        if raw_b64.strip().endswith("null"):
            return "no data in this page — it is the drop-target viewer, nothing to boot"
        raw = list(gzip.decompress(base64.b64decode(json.loads(raw_b64))))
    cut = src.rindex("(async function()")
    js = ("(function(){\nvar IDS = " + json.dumps(ids) + ";\n"
          + "var OPENIDS = " + json.dumps(open_ids) + ";\n" + STUB + src[:cut] +
          ("\nMETA = " + json.dumps(meta) + ";\nSOURCE = "
           + json.dumps(pathlib.Path(rowver).name) + ";" if rowver else "") +
          "\nvar buf = new Uint8Array(" + json.dumps(raw) + ").buffer;"
          "\ntry { unpack(buf); } catch(e){ return 'unpack THREW: ' + e.message; }"
          "\ntry { showSource(); } catch(e){ return 'showSource THREW: ' + e.message; }"
          "\ntry { buildUI(); } catch(e){ return 'buildUI THREW: ' + e.message; }"
          "\ntry { buildSchema(); } catch(e){ return 'buildSchema THREW: ' + e.message; }"
          # every column, plus weight, plus dataset, gets a row. Counted by the `tyr`
          # class the rows now carry rather than by a bare <tr>: the byte-cost colouring
          # gives each row a class, and the header row is not a data row anyway.
          "\nvar srows = (DOC.schema.innerHTML.match(/<tr class=\"tyr /g) || []).length;"
          # + dataset always, + weights only when the file has any
          "\nif (srows !== META.cols.length + (WCOL ? 2 : 1))"
          "  return 'schema has ' + srows + ' data rows for ' + META.cols.length + ' columns';"
          # the size arithmetic under that table is only worth showing if it is RIGHT:
          # width per event times events must equal the byte count the header declares
          "\nvar SZ = 'skipped';"
          "\nif (typeof sizeRows === 'function'){"
          "  var W9 = {f8:8, i4:4};"
          "  var per9 = META.cols.reduce(function(a, c){ return a + W9[c.kind]; }, 0)"
          "           + (WCOL ? 8 : 0) + 4;"
          "  if (per9 * META.n !== META.bytes)"
          "    return 'the column-size arithmetic gives ' + (per9 * META.n)"
          "         + ' but the header declares ' + META.bytes;"
          "  if (sizeRows().join(' ').indexOf('wrongly') >= 0)"
          "    return 'the page says its own size arithmetic disagrees with the header';"
          "  if ((DOC.schema.innerHTML.match(/schsum/g) || []).length !== 3)"
          "    return 'the size summary is not under the column table';"
          "  SZ = per9 + 'B/event';"
          "}"
          "\ntry { PLOTS=[newPlot()]; FACTORY=JSON.parse(JSON.stringify(collectState()));"
          "       initTabs(); applyState(TABS[TAB].st); buildTabs(); renderPlots(); }"
          "  catch(e){ return 'state/render THREW: ' + e.message; }"
          "\ntry { drawAll(); } catch(e){ return 'drawAll THREW: ' + e.message; }"
          "\nvar t; try { t = cutText(); } catch(e){ return 'cutText THREW: ' + e.message; }"
          # The property the whole .rvr round trip rests on: applying a collected state
          # must reproduce it exactly. Mutate first, so this cannot pass by nothing changing.
          "\nvar A = JSON.stringify(collectState());"
          "\nvar g0X = S.group[0];"
          "\nPLOTS[0].nb = 7; S.lumi = 137; S.show[0] = false; S.group[0] = 'X';"
          "\nvar B = collectState();"
          "\nif (B.plots[0].nb !== 7 || B.lumi !== 137 || B.show[META.datasets[0]] !== false)"
          "  return 'collectState did not see a change that was made';"
          "\nif (groupingDict().group[META.datasets[0]] !== 'X')"
          "  return 'the grouping dict did not see a group change';"
          "\nif (JSON.stringify(B) === A) return 'collectState returned the same state after a change';"
          "\nS.group[0] = g0X || null;"
          "\ntry { applyState(JSON.parse(A)); } catch(e){ return 'applyState THREW: ' + e.message; }"
          "\nif (JSON.stringify(collectState()) !== A)"
          "  return 'collect(apply(x)) !== x — state does not survive a round trip';"
          # A group drawn with members hidden must say so in the trace name. This is the one
          # failure that looks completely fine on screen, so it gets an assertion.
          "\nvar G = groupNames().filter(function(g){ return members(g).length > 1; });"
          "\nvar PARTIAL = 'no multi-sample group to test';"
          "\nif (G.length){"
          "  var m = members(G[0]);"
          "  var vis = m.filter(function(j){ return S.show[j] !== false; });"
          "  if (vis.length > 1){"
          # the label lives in a 1D trace name; a 2D panel carries it in its title instead,
          # so force 1D rather than reading null off a heatmap
          "    var wasMode = PLOTS[0].mode; PLOTS[0].mode = '1d';"
          # drawAll refreshes MASK and leaves DRAWN holding the LAST plot, so draw the one
          # under test afterwards
          "    S.show[vis[0]] = false; drawAll(); drawOne(PLOTS[0], MASK);"
          # the band must show the halfway hatch, not a dead chip, for a partial family
          "    buildSamples();"
          "    if (DOC.dslist.innerHTML.indexOf('aria-pressed=\"mixed\"') < 0)"
          "      return 'a partial group does not show the halfway state';"
          "    var want = '[' + (vis.length-1) + ' of ' + m.length + ']';"
          "    if (!DRAWN.some(function(t){ return t.name && t.name.indexOf(want) >= 0; }))"
          "      return 'a partial group drew without saying so: ' +"
          "             JSON.stringify(DRAWN.map(function(t){ return t.name; }));"
          "    if (cutText().indexOf(want) < 0) return 'cutText hid the partial group';"
          "    PARTIAL = 'ok';"
          "    S.show[vis[0]] = true; PLOTS[0].mode = wasMode; drawAll();"
          "  }"
          "}"
          # the pull panel and log binning, which nothing else on this page exercises
          "\nvar PULL = 'no fom pair';"
          "\nvar keepMode = PLOTS[0].mode; PLOTS[0].mode = '1d';"   # pull is a 1D panel
          "\nPLOTS[0].pull = true; PLOTS[0].lbx = true;"
          "\ntry { drawOne(PLOTS[0], MASK); }"
          "  catch(e){ return 'pull/log-bin THREW: ' + e.message; }"
          "\nif (S.fomn && S.fomd){"
          "  var pt = DRAWN.filter(function(t){ return t.name === 'pull'; })[0];"
          "  if (!pt) return 'pull was on and no pull trace was drawn';"
          "  if (pt.y.length !== PLOTS[0].nb) return 'pull has ' + pt.y.length + ' of ' + PLOTS[0].nb + ' bins';"
          "  PULL = 'ok';"
          "}"
          "\nPLOTS[0].pull = false; PLOTS[0].lbx = false; PLOTS[0].mode = keepMode;"
          "\ndrawOne(PLOTS[0], MASK);"
          # a cut typed past either end used to run the fill bar off the panel
          "\nvar ci = META.cols.findIndex(function(c){ return !c.vals; });"
          "\nif (ci >= 0){"
          "  var was = S.cuts[ci].slice();"
          "  S.cuts[ci] = [bounds(META.cols[ci])[0] - 1e6, bounds(META.cols[ci])[1] + 1e6];"
          "  cutLabels();"
          "  var L = parseFloat(DOC['fl'+ci].style.left), W = parseFloat(DOC['fl'+ci].style.width);"
          "  if (!(L >= 0 && W >= 0 && L + W <= 100.0001))"
          "    return 'the cut fill bar left the track: left=' + L + ' width=' + W;"
          "  S.cuts[ci] = was; cutLabels();"
          "}"
          # the zip writer is hand-rolled from the spec, so its bytes get checked
          "\nvar Z; try { Z = zipBytes([['a.txt', new Uint8Array([104,105])]]); }"
          "  catch(e){ return 'zipBytes THREW: ' + e.message; }"
          "\nvar dv = new DataView(Z.buffer);"
          "\nif (dv.getUint32(0, true) !== 0x04034b50) return 'zip has no local file header';"
          "\nif (dv.getUint32(Z.length-22, true) !== 0x06054b50) return 'zip has no end record';"
          "\nif (crc32(new Uint8Array([104,105])) !== 0xd8932aac)"
          "  return 'crc32 disagrees with the known value for \"hi\"';"
          # "export everything" bundles per-tab plots + cuts + the .rvr under one folder.
          # bundle() is pure so it can be checked here; nothing awaited can be, JSC
          # drains no microtasks.
          "\nvar EV; try {"
          "  var shots = PLOTS.map(function(p, i){"
          "      return ['p' + i + '.png', new Uint8Array([137,80,78,71])]; });"
          "  var CUTB = new TextEncoder().encode(cutText());"
          "  EV = bundle([[TABS[TAB].name, shots, CUTB]], new Uint8Array([31,139,8]),"
          "              stem() + '_STAMP');"
          "} catch(e){ return 'bundle THREW: ' + e.message; }"
          "\nif (EV.length !== PLOTS.length + 2)"
          "  return 'bundle made ' + EV.length + ' entries for ' + PLOTS.length + ' plots';"
          "\nvar root = stem() + '_STAMP/';"
          "\nif (!/^\\d{4}-\\d{2}-\\d{2}_\\d{2}-\\d{2}-\\d{2}$/.test(stamp()))"
          "  return 'stamp() is not a sortable filename-safe timestamp: ' + stamp();"
          "\nif (!EV.every(function(f){ return f[0].indexOf(root) === 0; }))"
          "  return 'a bundle entry escaped ' + root + ': ' + EV.map(function(f){return f[0];}).join();"
          "\nvar enames = EV.map(function(f){ return f[0]; });"
          "\nif (new Set(enames).size !== enames.length)"
          "  return 'bundle repeated a filename: ' + enames.join();"
          "\nif (enames.indexOf(root + 'cuts.py') < 0) return 'bundle has no cuts.py';"
          "\nif (enames.indexOf(root + stem() + '_STAMP.rvr') < 0) return 'bundle has no .rvr';"
          "\nvar cutsEntry = EV.filter(function(f){ return f[0] === root + 'cuts.py'; })[0][1];"
          "\nif (cutsEntry.length !== CUTB.length)"
          "  return 'bundle mangled the cuts bytes';"
          # several tabs get folders named after themselves — sanitized and de-duped
          "\nvar EV2 = bundle([['a', shots, CUTB], ['a/b', shots, CUTB], ['a_b', shots, CUTB]],"
          "                  new Uint8Array([1]), 'B');"
          "\nvar n2 = EV2.map(function(f){ return f[0]; });"
          "\nif (n2.indexOf('B/a/cuts.py') < 0 || n2.indexOf('B/a_b/cuts.py') < 0"
          "    || n2.indexOf('B/a_b_2/cuts.py') < 0 || n2.indexOf('B/B.rvr') < 0)"
          "  return 'multi-tab bundle layout is wrong: ' + n2.join();"
          "\nif (new Set(n2).size !== n2.length) return 'multi-tab bundle repeated a filename';"
          # cuts locked across tabs hoist to ONE shared_cuts.py at the top
          "\nvar EVS = bundle([['a', shots, CUTB], ['b', shots, CUTB]], new Uint8Array([1]), 'S', CUTB);"
          "\nvar ns = EVS.map(function(f){ return f[0]; });"
          "\nif (ns.indexOf('S/shared_cuts.py') < 0) return 'locked cuts did not hoist to shared_cuts.py';"
          "\nif (ns.some(function(x){ return x.indexOf('cuts.py') >= 0 && x !== 'S/shared_cuts.py'; }))"
          "  return 'shared cuts still copied per tab';"
          "\ntry { zipBytes(EV); } catch(e){ return 'zipBytes(bundle) THREW: ' + e.message; }"
          # grouping is PAGE-global: a move rides no tab state, FACTORY cannot clear it,
          # and a fresh tab sees it — that is the whole point of the band
          "\nvar MOVE = 'no groups';"
          "\nif (groupNames().length){"
          "  var g0 = groupNames()[0], was = members(g0).slice();"
          "  if (was.length){"
          "    moveTo([was[0]], null);"
          "    if (members(g0).length !== was.length - 1) return 'moveTo did not remove it';"
          "    if (groupNames().indexOf(g0) < 0) return 'emptying a group deleted the row';"
          "    var stM = collectState();"
          "    if ('group' in stM || 'sub' in stM || 'order' in stM)"
          "      return 'grouping leaked into a tab state';"
          "    applyState(FACTORY);"
          "    if (members(g0).length !== was.length - 1)"
          "      return 'FACTORY application clobbered the global grouping';"
          "    applyState(stM);"
          "    var tM = TAB;"
          "    newTab(false);"
          "    if (members(g0).length !== was.length - 1) return 'a fresh tab lost the move';"
          "    switchTab(tM); closeTab(TABS.length - 1);"
          "    moveTo(was, g0); MOVE = 'ok';"
          "  }"
          "}"
          # the cutflow and the readout must agree about the same events, or one is lying
          "\nvar cf; try { buildTables(); cf = cutflow(); }"
          "  catch(e){ return 'cutflow THREW: ' + e.message; }"
          "\nvar lastRow = cf.rows[cf.rows.length-1], tt = totals(MASK);"
          "\nif (lastRow.raw !== tt.kept)"
          "  return 'cutflow ends at ' + lastRow.raw + ' but the readout shows ' + tt.kept;"
          # every cut the mask applies must be a step, or the cutflow is quietly incomplete
          "\nvar nAct = 0; META.cols.forEach(function(c, q){ if (narrowed(q)) nAct++; });"
          "\nif (cf.steps.length - 1 !== nAct)"
          "  return 'the mask applies ' + nAct + ' cuts but the cutflow shows ' + (cf.steps.length-1);"
          # creating a group, filling it by drag, and exporting it
          "\nvar NEWG = 'skipped';"
          "\nif (META.datasets.length){"
          "  var n0 = groupNames().length;"
          "  DOC.newgrp.value = '_probe_'; DOC.addgrp.onclick();"
          "  if (groupNames().length !== n0 + 1) return 'new group was not created';"
          "  DOC.newgrp.value = '_probe_'; DOC.addgrp.onclick();"
          "  if (groupNames().length !== n0 + 1) return 'a duplicate group name was accepted';"
          "  DOC.newgrp.value = '  '; DOC.addgrp.onclick();"
          "  if (groupNames().length !== n0 + 1) return 'a blank group name was accepted';"
          # dataset 0's own group is put back at the end: removeGroup ungroups its members
          # by design, and leaving it loose emptied the signal group of any study with one
          # sample in it, which then silently disabled every later pull assertion
          "  var g0was = S.group[0] || null;"
          "  moveTo([0], '_probe_');"
          "  if (members('_probe_').length !== 1) return 'could not move a sample into it';"
          # a group is a folder and ships CLOSED — its own chevron deals the samples out
          "  if (DOC.dslist.innerHTML.indexOf('data-s=\"0\"') >= 0)"
          "    return 'a fresh group sprawls its samples';"
          "  toggleGrp('_probe_');"
          "  if (DOC.dslist.innerHTML.indexOf('data-s=\"0\"') < 0)"
          "    return 'unfolding the group did not deal out its samples';"
          "  toggleGrp('_probe_');"
          "  var mg = tunedMeta().grouping;"
          "  if (!mg || (mg.order || []).indexOf('_probe_') < 0)"
          "    return 'the new group is missing from the saved META';"
          "  if (mg.group[META.datasets[0]] !== '_probe_')"
          "    return 'the saved grouping does not carry the moved sample';"
          "  var stg = collectState();"
          "  applyState(FACTORY); applyState(stg);"
          "  if (members('_probe_').length !== 1) return 'a state swap clobbered the new group';"
          # nesting: a group dropped on a group becomes ONE bundle inside it; dragged to
          # the pool it is a group again; its × unbundles in place; and a bundle answers
          # as a table source by its own name
          "  DOC.newgrp.value = '_probe2_'; DOC.addgrp.onclick();"
          "  if (groupNames().indexOf('_probe2_') < 0) return 'the second group was not created';"
          "  nestGroup('_probe_', '_probe2_');"
          "  if (groupNames().indexOf('_probe_') >= 0) return 'nesting left the group at top level';"
          "  if (S.group[0] !== '_probe2_' || S.sub[0] !== '_probe_')"
          "    return 'nesting did not bundle the sample';"
          "  toggleGrp('_probe2_');"   # the outer folder opens; the bundle inside stays shut
          "  if (DOC.dslist.innerHTML.indexOf('_probe2_') < 0"
          "      || DOC.dslist.innerHTML.indexOf('data-sg=\"_probe_\"') < 0)"
          "    return 'the nested bundle is not drawn as a cluster';"
          # the bundle's dashed box is its own drop target — finer resolution than the row
          "  if (DOC.dslist.innerHTML.indexOf('data-subdrop=\"_probe_\"') < 0)"
          "    return 'the bundle is not a drop target';"
          # a bundle ships folded like a closed folder; the chevron deals the samples
          # out and tucks them back
          "  if (DOC.dslist.innerHTML.indexOf('data-s=\"0\"') >= 0)"
          "    return 'a folded bundle sprawls its samples';"
          "  toggleSub('_probe_', '_probe2_');"
          "  if (DOC.dslist.innerHTML.indexOf('data-s=\"0\"') < 0)"
          "    return 'unfolding did not deal out the samples';"
          "  toggleSub('_probe_', '_probe2_');"
          "  if (DOC.dslist.innerHTML.indexOf('data-s=\"0\"') >= 0)"
          "    return 'refolding did not tuck the samples back';"
          "  var mg2 = tunedMeta().grouping;"
          "  if (mg2.sub[META.datasets[0]] !== '_probe_' || mg2.group[META.datasets[0]] !== '_probe2_')"
          "    return 'the nested bundle is missing from the saved META';"
          "  if (typeof tblIdx === 'function'"
          "      && (tblIdx('_probe_').length !== 1 || tblIdx('_probe_')[0] !== 0))"
          "    return 'the bundle does not answer as a table source';"
          "  unnest('_probe_', [0]);"
          "  if (groupNames().indexOf('_probe_') < 0 || S.sub[0])"
          "    return 'unnesting did not remake the group';"
          "  nestGroup('_probe_', '_probe2_');"
          "  dissolveSub('_probe_', '_probe2_');"
          "  if (S.sub[0] || S.group[0] !== '_probe2_') return 'unbundling moved the sample';"
          "  moveTo([0], '_probe_');"
          "  removeGroup('_probe2_');"
          "  if (groupNames().indexOf('_probe2_') >= 0) return 'the outer group did not remove';"
          "  if (members('_probe_').length !== 1) return 'the rebuild lost the sample';"
          "  var loose = inGroup(null).length;"
          "  removeGroup('_probe_');"
          "  if (groupNames().indexOf('_probe_') >= 0) return 'removeGroup left the group behind';"
          "  if (inGroup(null).length !== loose + 1)"
          "    return 'removeGroup lost its sample instead of ungrouping it';"
          "  moveTo([0], g0was); drawAll();"
          "  if ((S.group[0] || null) !== g0was) return 'could not put dataset 0 back';"
          "  NEWG = 'ok';"
          "}"
          # a download name must never carry two extensions
          "\nvar keepSrc = SOURCE, keepTitle = META.title;"
          "\nSOURCE = 'x.rvr'; META.title = 'x.rvr';"
          "\nvar nm = stem() + '_tuned.rvr';"
          "\nSOURCE = ''; META.title = 'x.html';"
          "\nvar nm2 = stem() + '_tuned.rvr';"
          "\nSOURCE = keepSrc; META.title = keepTitle;"
          "\nif (nm !== 'x_tuned.rvr' || nm2 !== 'x_tuned.rvr')"
          "  return 'export names doubled an extension: ' + nm + ' / ' + nm2;"
          # and never stack stamps: saving what you reloaded must shed the old one first
          "\nSOURCE = 'y_2026-08-11_16-45-18.rvr';"
          "\nvar nm3 = stem();"
          "\nSOURCE = 'y_2026-08-11_16-45-18_2026-08-11_16-47-31.rvr';"
          "\nvar nm4 = stem();"
          "\nSOURCE = 'plain_name.rvr'; var nm5 = stem();"
          "\nSOURCE = keepSrc;"
          "\nif (nm3 !== 'y' || nm4 !== 'y' || nm5 !== 'plain_name')"
          "  return 'stem does not shed old stamps: ' + [nm3, nm4, nm5].join(' / ');"
          # 3D must be the same grid as 2D, or it is a different plot wearing its name
          "\nvar D3 = 'skipped';"
          "\nvar pm = PLOTS[0].mode, pv = PLOTS[0].view;"
          "\nPLOTS[0].view = 'one';"
          "\nPLOTS[0].mode = '2d'; drawOne(PLOTS[0], MASK);"
          "\nvar z2 = JSON.stringify(DRAWN[0].z), k2 = DRAWN[0].type;"
          "\nPLOTS[0].mode = '3d'; PLOTS[0].d3style = 'surface'; drawOne(PLOTS[0], MASK);"
          "\nvar z3 = JSON.stringify(DRAWN[0].z), k3 = DRAWN[0].type;"
          "\nif (k2 !== 'heatmap' || k3 !== 'surface')"
          "  return '2D / 3D-surface trace types are ' + k2 + '/' + k3;"
          "\nif (z2 !== z3) return '3D surface is not drawing the same grid as 2D';"
          # the default 3D is a mesh of boxes, one per non-empty bin
          "\nPLOTS[0].d3style = 'bars'; drawOne(PLOTS[0], MASK);"
          "\nvar bt = DRAWN[0];"
          "\nif (bt.type !== 'mesh3d') return 'the default 3D style is ' + bt.type;"
          "\nvar boxes = bt.x.length / 8;"
          "\nif (bt.i.length !== boxes * 12 || bt.z.length !== boxes * 8)"
          "  return 'the 3D histogram mesh is malformed: ' + boxes + ' boxes, '"
          "         + bt.x.length + ' verts, ' + bt.i.length + ' tris';"
          "\nvar filled = 0, gz = JSON.parse(z2);"
          "\nfor (var gy = 0; gy < gz.length; gy++)"
          "  for (var gx = 0; gx < gz[gy].length; gx++) if (gz[gy][gx] !== null) filled++;"
          "\nif (boxes !== filled)"
          "  return 'the mesh has ' + boxes + ' boxes for ' + filled + ' filled bins';"
          # panels side by side are only comparable if they share x and y
          "\nvar pv0 = PLOTS[0].view;"
          "\nPLOTS[0].view = 'three'; drawOne(PLOTS[0], MASK);"
          "\nvar sc = [LAY.scene, LAY.scene2, LAY.scene3];"
          "\nfor (var q = 1; q < 3; q++){"
          "  if (String(sc[q].xaxis.range) !== String(sc[0].xaxis.range))"
          "    return '3D panel ' + (q+1) + ' has a different x range';"
          "  if (String(sc[q].yaxis.range) !== String(sc[0].yaxis.range))"
          "    return '3D panel ' + (q+1) + ' has a different y range';"
          "}"
          "\nif (String(sc[1].zaxis.range) !== String(sc[0].zaxis.range))"
          "  return 'the two count panels have different z ranges';"
          "\nif (sc[2].zaxis.range) return 'the comparison panel was pinned to the yield z range';"
          "\nif (DRAWN[2].type !== DRAWN[0].type)"
          "  return 'the comparison panel ignored the 3D style: ' + DRAWN[2].type;"
          # the CMS row must clear the panel titles rather than sit on top of them
          "\nvar cmsY = null, titY = null;"
          "\nLAY.annotations.forEach(function(a){"
          "  var t = a.text.replace(/<[^>]+>/g, '');"
          "  if (t === 'CMS' || t === S.cms) cmsY = (cmsY === null) ? a.y : Math.min(cmsY, a.y);"
          "  else titY = (titY === null) ? a.y : Math.max(titY, a.y); });"
          "\nif (cmsY !== null && titY !== null && cmsY <= titY)"
          "  return 'the CMS label sits on the panel titles: CMS y=' + cmsY + ' titles y=' + titY;"
          # the right-hand label belongs over the plot, not over the colour bar
          "\nDOC.rlabel.value = '13.6 TeV'; drawOne(PLOTS[0], MASK);"
          "\nvar rl = LAY.annotations.filter(function(a){ return a.text.indexOf('13.6') >= 0; })[0];"
          "\nvar barx = DRAWN.filter(function(t){ return t.colorbar; })"
          "                  .map(function(t){ return t.colorbar.x; });"
          # the rightmost bar is the one it can collide with; a mid-figure bar between two
          # panels sits to the LEFT of the label and is not a collision
          "\nif (rl && barx.length && rl.x >= Math.max.apply(null, barx))"
          "  return 'the right-hand label is out past the plot: x=' + rl.x + ' bars at ' + barx;"
          "\nDOC.rlabel.value = '';"
          "\nPLOTS[0].view = pv0;"
          # the pull axis takes a manual range, on the same rule as x/y/z: both ends or none
          "\nvar PR = 'skipped';"
          "\nvar fomLive = S.fomn && S.fomd"
          "  && members(S.fomn).some(function(i){ return S.show[i] !== false; })"
          "  && members(S.fomd).some(function(i){ return S.show[i] !== false; });"
          "\nif (PULL !== 'no fom pair' && fomLive){"
          "  var wp = PLOTS[0].pull, wm = PLOTS[0].mode;"
          "  PLOTS[0].mode = '1d'; PLOTS[0].pull = true;"
          "  PLOTS[0].pmin = ''; PLOTS[0].pmax = '';"
          "  drawAll(); drawOne(PLOTS[0], MASK);"
          "  if (!LAY.yaxis2) return 'pull is on but there is no second y axis';"
          "  if (LAY.yaxis2.range) return 'the pull axis is pinned with no range given';"
          "  PLOTS[0].pmin = '-5'; drawOne(PLOTS[0], MASK);"
          "  if (LAY.yaxis2.range) return 'one end alone pinned the pull axis';"
          "  PLOTS[0].pmax = '2'; drawOne(PLOTS[0], MASK);"
          "  if (!LAY.yaxis2.range || LAY.yaxis2.range[0] !== -5 || LAY.yaxis2.range[1] !== 2)"
          "    return 'the pull axis ignored its range: ' + JSON.stringify(LAY.yaxis2.range);"
          "  PLOTS[0].pmin = 'abc'; drawOne(PLOTS[0], MASK);"
          "  if (LAY.yaxis2.range) return 'an unparseable pull min still pinned the axis';"
          "  PLOTS[0].pmin = '-5';"
          "  if (collectState().plots[0].pmax !== '2') return 'pmax did not reach the export';"
          "  PR = 'ok'; PLOTS[0].pull = wp; PLOTS[0].mode = wm;"
          "}"
          # `shape` divides each curve by its own sum, so the y axis must stop naming a
          # yield — and the lumi factor cancels, so it must not name that either
          "\nvar NORM = 'skipped';"
          "\nvar wasN = PLOTS[0].norm, wasM = PLOTS[0].mode;"
          "\nPLOTS[0].mode = '1d'; PLOTS[0].norm = 'yield'; drawAll(); drawOne(PLOTS[0], MASK);"
          "\nvar tY = LAY.yaxis.title.text;"
          "\nif (/normalis/i.test(tY)) return 'the yield y axis claims to be normalised: ' + tY;"
          "\nPLOTS[0].norm = 'shape'; drawOne(PLOTS[0], MASK);"
          "\nvar tS = LAY.yaxis.title.text;"
          "\nif (!/normalis/i.test(tS)) return 'the shape y axis does not say normalised: ' + tS;"
          "\nif (tS === tY) return 'shape and yield share a y axis title: ' + tS;"
          "\nif (/[0-9]/.test(tS)) return 'the shape y axis names a scale that cancels: ' + tS;"
          "\nvar curves = DRAWN.filter(function(t){ return t.name !== 'pull'; });"
          "\nif (!curves.length) return 'shape drew no curves';"
          "\nfor (var q = 0; q < curves.length; q++){"
          "  var sm = 0; for (var w = 0; w < curves[q].y.length; w++) sm += curves[q].y[w];"
          "  if (sm && Math.abs(sm - 1) > 1e-9)"
          "    return 'shape curve ' + curves[q].name + ' sums to ' + sm + ', not 1';"
          "}"
          "\nNORM = curves.length + 'curves';"
          "\nPLOTS[0].norm = wasN; PLOTS[0].mode = wasM;"
          # the stack toggle (0.33.0): backgrounds stack cumulatively — the drawn top
          # of the stack must equal the flat curves' per-bin sum EXACTLY (same arrays,
          # both modes) — signals overlay un-filled, and off restores the flat draw.
          "\nvar STK = 'skipped';"
          "\nif (PLOTS.length){"
          "  var sp9 = PLOTS[0], was9 = {mode:sp9.mode, stack:sp9.stack, norm:sp9.norm};"
          "  sp9.mode = '1d'; sp9.norm = ''; sp9.stack = true; drawOne(sp9, MASK);"
          "  var st9 = DRAWN.filter(function(t){ return t.fill; });"
          "  var ov9 = DRAWN.filter(function(t){ return !t.fill && t.name !== 'pull'; });"
          "  if (!st9.length) return 'stack on drew no filled trace';"
          "  var topY9 = st9[st9.length-1].y, names9 = st9.map(function(t){ return t.name; });"
          "  sp9.stack = false; drawOne(sp9, MASK);"
          "  var sum9 = topY9.map(function(){ return 0; }), seen9 = 0;"
          "  DRAWN.forEach(function(t){ if (names9.indexOf(t.name) < 0) return; seen9++;"
          "    t.y.forEach(function(v, i){ sum9[i] += v; }); });"
          "  if (seen9 !== names9.length)"
          "    return 'flat draw lost ' + (names9.length - seen9) + ' stacked curve(s)';"
          "  var worst9 = 0;"
          "  topY9.forEach(function(v, i){ worst9 = Math.max(worst9, Math.abs(v - sum9[i])); });"
          "  if (worst9 > 1e-9) return 'stack top differs from the component sum by ' + worst9;"
          "  sp9.mode = was9.mode; sp9.norm = was9.norm;"
          "  if (was9.stack) sp9.stack = true; else delete sp9.stack;"
          "  drawOne(sp9, MASK);"
          "  STK = st9.length + 'stk/' + ov9.length + 'over';"
          "}"
          # the wordmark easter egg (0.30.0, the optimizer's replacement): three fast
          # clicks give Rover a top hat, two never do, and the hat is pure posture —
          # the page state cannot change and nothing of it is ever saved.
          "\nvar HAT = 'no handler';"
          "\nif (typeof RV === 'object' && DOC.bname.onclick){"
          "  var beforeHat = JSON.stringify(collectState());"
          "  DOC.bname.onclick(); DOC.bname.onclick();"
          "  if (RV.hat) return 'two clicks put the hat on';"
          "  DOC.bname.onclick();"
          "  if (!RV.hat) return 'three clicks did not put the hat on';"
          "  if (roverFrame(0.3, false, true).indexOf('_____') < 0)"
          "    return 'the hat frame draws no crown';"
          "  if (roverFrame(0.3, false, false).indexOf('_') >= 0)"
          "    return 'a bare head still shows hat glyphs';"
          "  DOC.bname.onclick(); DOC.bname.onclick(); DOC.bname.onclick();"
          "  if (RV.hat) return 'three more clicks did not take the hat off';"
          "  if (JSON.stringify(collectState()) !== beforeHat)"
          "    return 'the hat leaked into the page state';"
          "  HAT = 'ok';"
          "}"
          # titles are editable in place; the edit is plot state, a zoom is not an edit,
          # and the CMS row is not a title anyone may retype here
          "\nvar ED = 'no handler';"
          "\nvar edv = DOC['plot' + PLOTS[0].id];"
          "\nif (edv && edv._h && edv._h['plotly_relayout']){"
          "  var wasx = PLOTS[0].xtitle, wasm2 = PLOTS[0].mode;"
          "  PLOTS[0].mode = '1d'; drawAll(); drawOne(PLOTS[0], MASK);"
          "  edv._h['plotly_relayout']({'xaxis.title.text': 'RETYPED'});"
          "  if (PLOTS[0].xtitle !== 'RETYPED') return 'an axis title edit was not stored';"
          "  if (LAY.xaxis.title.text !== 'RETYPED') return 'the edited axis title was not drawn';"
          "  if (collectState().plots[0].xtitle !== 'RETYPED') return 'the edit did not export';"
          "  edv._h['plotly_relayout']({'xaxis.range[0]': 0.1});"
          "  if (PLOTS[0].xtitle !== 'RETYPED') return 'a zoom was mistaken for a title edit';"
          "  if (CMSN[PLOTS[0].id] === undefined) return 'the CMS annotation count was never recorded';"
          "  edv._h['plotly_relayout']({'annotations[0].text': 'NOPE'});"
          "  if (JSON.stringify(LAY.annotations).indexOf('NOPE') >= 0)"
          "    return 'the CMS row accepted an edit';"
          "  PLOTS[0].xtitle = wasx; PLOTS[0].mode = wasm2; ED = 'ok';"
          "}"
          # the colour bar names what it counts and nothing else; the ticks carry the scale
          # A plot's caveat is a BADGE with the words in its tooltip, not text in the
          # header: as text a long one wrapped the header taller and knocked two plots in
          # a sheet out of line. Force a real overflow rather than hope for one.
          "\nvar NOTE = 'skipped';"
          # a page with ONE plot may have it in 2D at this point, so set the mode rather
          # than hunt for a 1D plot — otherwise the check quietly skips exactly there
          "\nvar nk = PLOTS[0];"
          "\nif (nk){"
          "  var kmd = nk.mode; nk.mode = '1d';"
          "  var kxa = nk.xauto, kxi = nk.xmin, kxa2 = nk.xmax, kof = nk.oflow;"
          "  nk.oflow = true;"
          "  var ci = META.cols.map(function(c){ return c.name; }).indexOf(nk.varx);"
          "  var col = META.cols[ci], sp9 = col.nice[1] - col.nice[0];"
          "  nk.xauto = false;"
          "  nk.xmin = col.nice[0] + sp9*0.45; nk.xmax = col.nice[0] + sp9*0.55;"
          "  drawAll();"
          "  var loud = NOTES[nk.id] || '';"
          # count what is genuinely outside the drawn window with a straight loop, so the
          # check cannot false-fail on a column that happens to fit inside it
          "  var vals9 = D[nk.varx], outn9 = 0;"
          "  for (var r9 = 0; r9 < META.n; r9++)"
          "    if (MASK[r9] && S.show[D.__ds[r9]] !== false"
          "        && (vals9[r9] < nk.xmin || vals9[r9] > nk.xmax)) outn9++;"
          "  var says9 = loud.indexOf('drawn range') >= 0;"
          "  if (outn9 > 0 && !says9)"
          "    return outn9 + ' events sit outside the drawn range and the badge is silent';"
          "  if (outn9 === 0 && says9)"
          "    return 'the badge warns about a range nothing falls outside: ' + loud;"
          "  if (loud.indexOf('<') >= 0) return 'the badge tooltip carries markup: ' + loud;"
          "  nk.xauto = kxa; nk.xmin = kxi; nk.xmax = kxa2; nk.oflow = kof;"
          "  nk.mode = kmd; renderPlots(); drawAll();"
          "  NOTE = outn9 + 'outside';"
          "}"
          # Whether a plot's notes are OPEN is POSTURE as of 0.21.0, not part of the
          # analysis: a file full of noted plots opened with every note docked and every
          # plot squashed. The text is saved, the panel is not, and the button wears a
          # mark instead. This pins the inversion — a saved cmton must not reopen anything.
          "\nvar CMT = 'skipped';"
          "\nif (PLOTS[0]){"
          "  var kc9 = PLOTS[0].cmt, ko9 = PLOTS[0].cmton;"
          "  PLOTS[0].cmt = 'a stored comment'; PLOTS[0].cmton = true;"
          "  var s9c = collectState().plots[0];"
          "  if (s9c.cmt !== 'a stored comment') return 'a comment did not reach the state';"
          "  if ('cmton' in s9c) return 'an open notes panel reached the saved state';"
          # a file written before the change carries cmton:true; it must be ignored
          "  var st9c = collectState();"
          "  st9c.plots[0].cmton = true;"
          "  applyState(st9c, false);"
          "  if (PLOTS[0].cmton) return 'a saved cmton reopened the notes on load';"
          "  if (PLOTS[0].cmt !== 'a stored comment') return 'the note text was lost on load';"
          "  PLOTS[0].cmt = kc9; PLOTS[0].cmton = ko9;"
          # applyState mints fresh plot objects with fresh ids, so the DOM has to be
          # rebuilt behind it or every later probe reaches for a div that is gone
          "  renderPlots(); drawAll();"
          "  CMT = 'text saved, panel posture';"
          "}"
          # Copy/paste a BLOCK — the unit the builder makes and the page lays out, one
          # solo plot or one sheet of them. What must be true: the copy is by VALUE (a
          # later edit to the source cannot reach the paste), the paste is a NEW block
          # rather than a merge into the source's sheet, and no cut rides along — cuts
          # belong to the tab, so a block adopts wherever it lands.
          # The colour panel, and specifically the things a JS reference error kills. Twice
          # now an edit deleted a top-level name the panel needs — gcol(), then PICKS — and
          # both times everything else stayed green because nothing CALLED the panel. This
          # calls it.
          "\nvar CLR = 'skipped';"
          "\nif (typeof colPanel === 'function'){"
          "  if (typeof PICKS === 'undefined' || !PICKS.length)"
          "    return 'the colour palette is gone';"
          "  if (PICKS.length !== 19) return 'the palette is ' + PICKS.length + ', not 19 + auto';"
          "  colPanel();"
          "  var ch9 = DOC.colpanel.innerHTML;"
          "  if (ch9.indexOf('colrow') < 0) return 'the colour panel rendered no rows';"
          "  if (ch9.indexOf('colcol') < 0) return 'the colour panel lost its columns';"
          "  var cg9 = groupNames()[0];"
          "  if (cg9){"
          "    var was9 = colours()[cg9];"
          "    setColour(cg9, PICKS[0]);"
          "    if (colours()[cg9] !== PICKS[0]) return 'a chosen colour did not beat the rotation';"
          "    if (srcColour(cg9) !== PICKS[0]) return 'srcColour disagreed with the choice';"
          # NOTHING inherits: a group names a family, but its samples are their own things.
          # This pinned the opposite until 2026-08-15, when setting one bundle repainted
          # every sample under it and Derek called it annoying.
          "    var mi9 = members(cg9)[0];"
          "    if (mi9 != null && srcColour(META.datasets[mi9]) === PICKS[0])"
          "      return 'a grouped sample was repainted by its group';"
          # and every sample has a colour of its own, grouped or loose
          "    for (var q8 = 0; q8 < META.datasets.length; q8++)"
          "      if (!srcColour(META.datasets[q8]))"
          "        return 'no colour for ' + META.datasets[q8];"
          "    setColour(cg9, null);"
          "    if (colours()[cg9] !== was9) return 'clearing did not restore the rotation';"
          "  }"
          "  CLR = 'ok';"
          "}"
          "\nvar CP = 'skipped';"
          "\nif (typeof copyBlock === 'function' && PLOTS.length){"
          "  var p9 = PLOTS[0], was9 = PLOTS.length;"
          "  p9.sheet = 77; p9.quad = 0; p9.nb = 23; p9.xtitle = 'original';"
          "  var mate9 = newPlot({sheet: 77, quad: 1, nb: 24});"
          "  PLOTS.push(mate9); renderPlots(); drawAll();"
          "  copyBlock(p9);"
          "  if (!CLIP || CLIP.kind !== 'plots' || CLIP.specs.length !== 2)"
          "    return 'copyBlock took ' + (CLIP ? JSON.stringify(CLIP.specs && CLIP.specs.length)"
          "                                     : 'nothing') + ' of a 2-plot sheet';"
          "  if (CLIP.specs.some(function(s){ return 'id' in s; }))"
          "    return 'a copied spec kept its id';"
          "  if (CLIP.specs.some(function(s){ return 'cuts' in s || 'chips' in s; }))"
          "    return 'a copied block carried cuts, which belong to the tab';"
          # copy by value: editing the source afterwards must not reach the clipboard
          "  p9.xtitle = 'edited after the copy';"
          "  if (CLIP.specs[0].xtitle !== 'original') return 'the clipboard aliased the live plot';"
          "  var n9 = PLOTS.length;"
          "  pasteBlock();"
          "  if (PLOTS.length !== n9 + 2) return 'paste added ' + (PLOTS.length - n9) + ' of 2';"
          # a paste SPENDS the clipboard: one copy, one paste, and the button goes with it
          "  if (CLIP) return 'the paste did not spend the clipboard';"
          "  if (!DOC.pasteblk.hidden) return 'the paste button outlived the clipboard';"
          "  var a9 = PLOTS[PLOTS.length - 2], b9 = PLOTS[PLOTS.length - 1];"
          "  if (a9.sheet === 77 || a9.sheet !== b9.sheet)"
          "    return 'the pasted pair did not land on one NEW sheet: ' + a9.sheet + '/' + b9.sheet;"
          "  if (a9.quad === b9.quad) return 'the pasted pair share a quadrant';"
          "  if (a9.nb !== 23 || b9.nb !== 24) return 'the pasted plots lost their settings';"
          "  if (a9.xtitle !== 'original') return 'the paste took an edit made after the copy';"
          "  if (a9.id === p9.id || b9.id === mate9.id) return 'a pasted plot reused an id';"
          # and pasting twice must not hand two blocks the same nested objects — which
          # needs a fresh copy now, since the first paste spent the last one
          "  copyBlock(p9);"
          "  pasteBlock();"
          "  var c9 = PLOTS[PLOTS.length - 2];"
          "  c9.titles.x = 'second paste only';"
          "  if (a9.titles && a9.titles.x === 'second paste only')"
          "    return 'two pastes share one titles object';"
          "  while (PLOTS.length > was9) PLOTS.pop();"
          "  p9.sheet = null; p9.quad = null; p9.xtitle = '';"
          "  CLIP = null; renderPlots(); drawAll();"
          "  CP = '2plots';"
          "}"
          "\nvar BAR = 'skipped';"
          "\nif (META.cols.length > 1){"
          "  var wasmb = PLOTS[0].mode, waslz = PLOTS[0].logz;"
          "  PLOTS[0].mode = '2d'; PLOTS[0].logz = true; drawAll(); drawOne(PLOTS[0], MASK);"
          "  var cbt = DRAWN.filter(function(t){ return t.colorbar; })[0];"
          "  if (!cbt) return 'a 2D plot drew no colour bar';"
          "  if (/log/i.test(cbt.colorbar.title.text))"
          "    return 'the colour bar announces the scale: ' + cbt.colorbar.title.text;"
          "  if (cbt.colorbar.tickmode !== 'array')"
          "    return 'log colour did not put real values on the bar ticks';"
          "  if (cbt.hovertemplate.indexOf(cbt.colorbar.title.text) < 0)"
          "    return 'the hover and the colour bar disagree on what z is';"
          "  BAR = 'ok'; PLOTS[0].logz = waslz; PLOTS[0].mode = wasmb;"
          "}"
          # a group's colour is ONE property: the readout number, the 1D curve and the
          # cutflow must all take it from colours(). The stub gives each CSS variable its
          # own value, without which every one of these comparisons passes trivially.
          "\nvar COL = 'no groups';"
          "\nif (groupNames().length){"
          "  var wasm = PLOTS[0].mode; PLOTS[0].mode = '1d';"
          "  drawAll(); drawOne(PLOTS[0], MASK);"
          "  var line = {}; DRAWN.forEach(function(t){ if (t.line) line[t.name] = t.line.color; });"
          "  var seenCols = {};"
          "  var gsc = groupNames().filter(function(g){"
          "    return members(g).some(function(i){ return S.show[i] !== false; }); });"
          "  for (var q = 0; q < gsc.length; q++){"
          "    var g = gsc[q], want = gcol(g);"
          "    if (!want) return 'group ' + g + ' has no colour';"
          "    if (seenCols[want]) return 'groups ' + seenCols[want] + ' and ' + g + ' share ' + want;"
          "    seenCols[want] = g;"
          "    var lc = line[g] || line[gLabel(g)];"
          "    if (lc !== want) return 'the 1D curve for ' + g + ' is ' + lc + ', not ' + want;"
          "    if (DOC.readout.innerHTML.indexOf('color:' + want) < 0)"
          "      return 'the readout does not carry ' + g + ' in ' + want;"
          "  }"
          "  COL = gsc.length + 'groups'; PLOTS[0].mode = wasm;"
          "}"
          # the two cut sections, their names, and that folding them survives an export
          "\nvar SEC = 'none';"
          "\nif (CUTSEC.cont || CUTSEC.cat){"
          "  var head = DOC.cutlist.innerHTML;"
          "  if (CUTSEC.cont && head.indexOf('Continuous cuts') < 0) return 'no Continuous cuts heading';"
          "  if (CUTSEC.cat && head.indexOf('Categorical cuts') < 0) return 'no Categorical cuts heading';"
          "  if (head.indexOf('Categories') >= 0) return 'the old Categories heading is still there';"
          # sidebars are page POSTURE, not state: no fold, drawer, theme or text-size
          # key may reach a save, and a file carrying the old keys is ignored politely
          "  var chromed = collectState();"
          "  if ('cutopen' in chromed || 'cntopen' in chromed || 'sampopen' in chromed"
          "      || 'drawer' in chromed || 'theme' in chromed || 'ts' in chromed)"
          "    return 'page posture leaked into the state';"
          "  if (chromed.plots.some(function(p){ return 'opts' in p; }))"
          "    return 'the per-plot options fold leaked into the state';"
          "  DOC.rail.hidden = true;"   # the stub does not parse markup `hidden`
          "  applyState(Object.assign({}, chromed, {drawer: true, sampopen: true, ts: 'lg'}));"
          "  if (!DOC.rail.hidden) return 'an old file posture key opened the drawer';"
          "  SEC = (CUTSEC.cont ? 'cont' : '') + (CUTSEC.cat ? '+cat' : '');"
          "}"
          # every comparison panel reads on its own, so each keeps tick labels; `matches`
          # still pins them to one range, and a bar in a panel's span would sit on the map
          "\nvar PAN = 'skipped';"
          "\nPLOTS[0].mode = '2d'; PLOTS[0].view = 'three'; PLOTS[0].cmpmode = 'ratio';"
          "\nPLOTS[0].layer = groupNames()[0] || 'all';"
          "\nPLOTS[0].layer2 = groupNames()[1] || 'all';"
          "\ndrawOne(PLOTS[0], MASK);"
          "\nvar dm = [], bars = [];"
          "\nfor (var q = 0; q < 3; q++){"
          "  var Y = LAY[q ? 'yaxis'+(q+1) : 'yaxis'], X = LAY[q ? 'xaxis'+(q+1) : 'xaxis'];"
          "  if (!Y || !X) return 'three-panel view built only ' + q + ' panels';"
          "  if (Y.showticklabels === false) return 'panel ' + (q+1) + ' has no y tick labels';"
          "  if (q && Y.matches !== 'y') return 'panel ' + (q+1) + ' y range is not pinned to panel 1';"
          "  dm.push(X.domain);"
          "}"
          "\nfor (var q = 1; q < 3; q++) if (dm[q][0] <= dm[q-1][1])"
          "  return 'panels ' + q + ' and ' + (q+1) + ' overlap: ' + JSON.stringify(dm);"
          "\nDRAWN.forEach(function(t){ if (t.showscale && t.colorbar) bars.push(t.colorbar.x); });"
          "\nfor (var b = 0; b < bars.length; b++) for (var q = 0; q < 3; q++)"
          "  if (bars[b] > dm[q][0] && bars[b] < dm[q][1])"
          "    return 'a colour bar at ' + bars[b] + ' sits on panel ' + (q+1);"
          "\nPAN = dm.length + 'panels';"
          "\nD3 = 'ok'; PLOTS[0].mode = pm; PLOTS[0].view = pv; drawOne(PLOTS[0], MASK);"
          # the cutflow y scale is a toggle and rides in the state
          "\nvar keepLog = S.cflog;"
          "\nS.cflog = 'lin'; buildTables();"
          "\nif (collectState().cflog !== 'lin') return 'cflog is missing from the state';"
          "\nS.cflog = keepLog; buildTables();"
          # an error banner must be dismissable, or a stray one sits there for good
          "\nfail('probe');"
          "\nif (DOC.prov.className !== 'prov bad') return 'fail() did not mark the line bad';"
          "\nif (!DOC.provx) return 'the error banner has no dismiss button';"
          "\nDOC.provx.onclick();"
          "\nif (DOC.prov.className !== 'prov')"
          "  return 'dismissing the error left the line as ' + DOC.prov.className;"
          "\nif (DOC.prov.innerHTML.indexOf('probe') >= 0) return 'the error text survived dismissal';"
          # the text-size toggle drives one attribute and rides in the state
          "\nvar keepTs = S.ts;"
          "\nS.ts = 'lg'; applyTextSize();"
          "\nif ('ts' in collectState()) return 'text size is posture and must not reach the state';"
          "\nS.ts = keepTs; applyTextSize();"
          # every plotly font goes through pf(); a literal left behind would not scale
          # A power-formatted log axis is drawn 1.25x larger by plotly, whole axis, so its
          # tick size is pre-divided to land on the same pixels as a linear one. Both are
          # still derived from pf(), which is what this guard is actually for.
          "\nvar tfs = LAY.xaxis.tickfont.size;"
          "\nvar want = LAY.xaxis.type === 'log' ? pf(11)/1.25 : pf(11);"
          "\nif (LAY.font.size !== pf(11) || tfs !== want)"
          "  return 'a plot font bypassed pf(): base=' + LAY.font.size"
          "         + ' tick=' + tfs + ' wanted ' + want;"
          "\nif (logAxis().tickfont.size !== pf(11)/1.25)"
          "  return 'the log tick size stopped compensating plotly 1.25x scaling';"
          "\nif (TFONT().size !== pf(12))"
          "  return 'the axis title font bypassed pf(): ' + TFONT().size;"
          # reordering cuts must change the LADDER and never the answer
          "\nvar CO = 'too few cuts';"
          "\nvar ac = activeCuts();"
          "\nif (ac.length > 1){"
          "  var was = cutflow(), lastWas = was.rows[was.rows.length-1];"
          "  moveCut(META.cols[ac[ac.length-1]].name, META.cols[ac[0]].name);"
          "  var now = cutflow(), lastNow = now.rows[now.rows.length-1];"
          "  if (JSON.stringify(now.steps) === JSON.stringify(was.steps))"
          "    return 'moveCut did not change the order';"
          "  if (lastNow.raw !== lastWas.raw)"
          "    return 'reordering changed the answer: ' + lastWas.raw + ' -> ' + lastNow.raw;"
          "  if (JSON.stringify(lastNow.by) !== JSON.stringify(lastWas.by))"
          "    return 'reordering changed the group yields';"
          "  if (collectState().cutorder.length !== ac.length)"
          "    return 'the cut order is missing from the state';"
          "  S.cutorder = []; buildTables(); CO = 'ok';"
          "}"
          "\ntry { resetCuts(); } catch(e){ return 'resetCuts THREW: ' + e.message; }"
          "\ntry { resetPlot(PLOTS[0]); } catch(e){ return 'resetPlot THREW: ' + e.message; }"
          "\nvar bits; try { bits = rowverBytes().parts; }"
          "  catch(e){ return 'rowverBytes THREW: ' + e.message; }"
          "\nvar L = new DataView(bits[0].buffer).getUint32(0, true);"
          "\nif (L !== bits[1].length) return 'header says ' + L + ' B, header is ' + bits[1].length;"
          "\nif (bits[2].byteLength !== META.bytes)"
          "  return 'body is ' + bits[2].byteLength + ' B, META says ' + META.bytes;"
          # tabs: empty and copied ones, switches that must not leak state, rename, close,
          # what tunedMeta writes, and the v2 single-state lift. Written against the tab
          # the file BOOTED on — a legitimate file may hold several tabs and open on any.
          # every rail fold ships CLOSED — applyState deliberately ignores the posture keys
          # a file may carry, so the markup is the only thing that decides, and an `open`
          # attribute there means every .rvr anyone ever opens starts with that fold down
          "\nvar FOLDS = 'skipped';"
          "\nif (DOC.sampbox){"
          "  var openf = ['sampbox','cntbox','contbox','catbox','tabsbox','expbox',"
          "               'fombox','labbox','dzbox'].filter(function(id){"
          "    return DOC[id] && DOC[id].open; });"
          "  if (openf.length) return 'these rail folds ship open: ' + openf.join(', ');"
          "  FOLDS = 'all closed';"
          "}"
          "\nvar TB = 'skipped';"
          "\nvar t0 = TABS.length, TAB0 = TAB;"
          "\nif (t0 !== ((META.tabs && META.tabs.length) || 1))"
          "  return 'boot made ' + t0 + ' tabs for ' + ((META.tabs && META.tabs.length) || 1) + ' in META';"
          "\nvar base = JSON.stringify(collectState());"
          "\nnewTab(false);"
          "\nif (TABS.length !== t0 + 1 || TAB !== t0) return 'newTab(empty) landed on tab ' + TAB;"
          # the drawer and rail folds are page chrome and survive tab clicks by design, and
          # a fresh tab is now EMPTY rather than the factory page — so it equals FACTORY in
          # everything but chrome and plots, and its plot list must be exactly []. That
          # emptiness is the point: st.plots = [] used to read as "no opinion" and leave the
          # last tab's plots standing, which is why applyState now tests the key, not length.
          "\nvar deChrome = function(st){ var c = JSON.parse(JSON.stringify(st));"
          "  ['drawer','sampopen','cutopen','catopen','cntopen','expopen','fomopen','cmsopen',"
          "   'plots'].forEach(function(k){ delete c[k]; }); return JSON.stringify(c); };"
          "\nif (deChrome(collectState()) !== deChrome(FACTORY))"
          "  return 'an empty tab differs from the factory page beyond its chrome and plots';"
          "\nif (collectState().plots.length !== 0) return 'a fresh tab arrived with plots on it';"
          "\nif (PLOTS.length !== 0) return 'a fresh tab drew plots';"
          # the tab needs something OF ITS OWN for the keep/copy checks below to mean
          # anything, and a fresh tab no longer comes with a plot — so build one, which is
          # what a reader does next anyway
          "\nS.lumi = 777; PLOTS.push(newPlot({nb: 13})); renderPlots(); drawAll();"
          "\nvar mine = JSON.stringify(collectState());"
          "\nswitchTab(TAB0);"
          "\nif (JSON.stringify(collectState()) !== base) return 'switching away corrupted the boot tab';"
          "\nswitchTab(t0);"
          "\nif (JSON.stringify(collectState()) !== mine) return 'a tab did not keep its own state';"
          "\nnewTab(true);"
          "\nif (JSON.stringify(collectState()) !== mine) return 'a copied tab differs from its source';"
          "\nS.lumi = 888;"
          "\nswitchTab(t0);"
          "\nif (S.lumi !== 777) return 'a copy shares state with its source';"
          "\nvar nm = 'renamed';"
          "\nwhile (TABS.some(function(x){ return x.name === nm; })) nm += ' x';"
          "\nrenameTab(TABS[TAB], nm);"
          "\nif (TABS[TAB].name !== nm) return 'rename did not take';"
          "\nvar tm = tunedMeta();"
          "\nif (tm.rowver !== ROWVER || !tm.tabs || tm.tabs.length !== TABS.length || 'state' in tm)"
          "  return 'tunedMeta wrote rowver=' + tm.rowver + ' tabs=' + (tm.tabs || []).length;"
          "\nif (tm.tab !== 0) return 'a save must open on the first tab, not #' + tm.tab;"
          "\nif (!/^[0-9]+\\./.test(String(tm.made_by)))"
          "  return 'the save carries no made_by version stamp';"
          # tabs rearrange by drag: order moves, the live tab stays the live tab
          "\nvar ord0 = TABS.map(function(x){ return x.name; }).join('|');"
          "\nvar curName = TABS[TAB].name;"
          "\nmoveTab(TABS.length - 1, 0);"
          "\nif (TABS.map(function(x){ return x.name; }).join('|') === ord0)"
          "  return 'moveTab moved nothing';"
          "\nif (TABS[TAB].name !== curName) return 'moveTab lost the live tab';"
          "\nmoveTab(0, TABS.length - 1);"
          "\nif (TABS.map(function(x){ return x.name; }).join('|') !== ord0)"
          "  return 'moveTab back did not restore the order';"
          "\ncloseTab(TABS.length - 1);"
          "\nif (S.lumi !== 777) return 'closing an inactive tab moved the view';"
          "\ncloseTab(TAB);"
          "\nswitchTab(TAB0);"
          "\nif (TABS.length !== t0 || TAB !== TAB0 || JSON.stringify(collectState()) !== base)"
          "  return 'closing tabs did not restore the boot tab';"
          "\nvar keepTabs = META.tabs, keepTab = META.tab, keepState = META.state;"
          "\nMETA.tabs = null; META.tab = 0; META.state = JSON.parse(base);"
          "\ninitTabs();"
          "\nif (TABS.length !== 1 || JSON.stringify(TABS[0].st) !== base) return 'the v2 lift broke';"
          "\nMETA.tabs = keepTabs; META.tab = keepTab; META.state = keepState;"
          "\ninitTabs(); applyState(FACTORY);"
          "\nif (TABS[TAB].st) applyState(TABS[TAB].st);"
          "\nbuildTabs(); drawAll();"
          "\nTB = 'ok';"
          # drawings + comments: the relayout listener stores a drawn shape, the store is
          # keyed by axes, the toggle hides it, the arrow lands, and all of it survives
          # collect -> apply
          "\nvar DR = 'skipped';"
          "\nvar pd = PLOTS[0];"
          "\nvar wasM = pd.mode, wasV = pd.view, wasP = pd.pull, wasX = pd.varx,"
          "    wasC = pd.compare, wasA = pd.anno;"
          "\npd.mode = '1d'; pd.pull = false; pd.view = 'one'; pd.compare = false; pd.anno = true;"
          "\ndrawOne(pd, MASK);"
          "\nvar dv = DOC['plot' + pd.id];"
          "\nif (!dv || !dv._h || !dv._h['plotly_relayout']) return 'no relayout listener on the plot div';"
          "\ndv.layout = LAY;"
          "\ndv._h['plotly_relayout']({shapes: [{type:'line', x0:1, x1:2, y0:0, y1:5,"
          "                                     xref:'x', yref:'y', line:{color:'#000', width:2}}]});"
          "\nvar dk = drawKey(pd);"
          "\nif (!pd.draw[dk] || pd.draw[dk].shapes.length !== 1 || pd.draw[dk].shapes[0].name !== 'rvdraw')"
          "  return 'a drawn shape did not reach the store';"
          "\ndrawOne(pd, MASK);"
          "\nif (!LAY.shapes || LAY.shapes.length !== 1) return 'the stored shape was not rendered';"
          "\ndv._h['plotly_relayout']({'shapes[0].x0': 1.5});"
          "\nif (pd.draw[dk].shapes[0].x0 !== 1.5) return 'a shape drag did not update the store';"
          "\ndv._h['plotly_relayout']({shapes: [pd.draw[dk].shapes[0],"
          "    {type:'line', x0:3, x1:4, y0:0, y1:1, xref:'x', yref:'y2', line:{color:'#000'}}]});"
          "\nif (pd.draw[dk].shapes.length !== 1) return 'an off-axes stroke reached the store';"
          "\npd.anno = false; drawOne(pd, MASK);"
          "\nif (LAY.shapes && LAY.shapes.length) return 'notes off still rendered drawings';"
          "\npd.anno = true;"
          "\nvar oc = META.cols.filter(function(c){ return c.name !== pd.varx; })[0];"
          "\nif (oc){"
          "  pd.varx = oc.name; drawOne(pd, MASK);"
          "  if (LAY.shapes && LAY.shapes.length) return 'drawings followed the plot onto other axes';"
          "  pd.varx = wasX; drawOne(pd, MASK);"
          "  if (!LAY.shapes || LAY.shapes.length !== 1) return 'drawings did not come back with their axes';"
          "}"
          "\nvar nn0 = pd.draw[dk].notes.length;"   # a file may ship with arrows already
          "\naddArrow(pd);"
          "\nif (pd.draw[dk].notes.length !== nn0 + 1) return 'the arrow did not land in the store';"
          "\nif (!LAY.annotations || !LAY.annotations.some(function(a){ return a.name === 'rvdraw'; }))"
          "  return 'the arrow was not rendered';"
          # A new arrow is anchored in DATA units on both axes, so a log flip on either one
          # moves it with its feature. Files written before that still say yref 'paper' and
          # must keep their old meaning — a fraction of the panel, never converted.
          "\nvar na = pd.draw[dk].notes[pd.draw[dk].notes.length - 1];"
          # x is always data. y is data too WHEN the axis can be measured — under this stub
          # it cannot, so yAt returns null and addArrow keeps the old paper meaning. What is
          # pinned here is that the pair always agrees; the browser rig pins the real default.
          "\nif (na.xref !== 'x') return 'a new arrow is not anchored in data x';"
          "\nif (na.yref === 'y'){ if (!(na.y > 1)) return 'a data-unit y that is not a count'; }"
          "\nelse if (na.yref === 'paper'){ if (!(na.y >= 0 && na.y <= 1))"
          "  return 'a paper y outside the panel'; }"
          "\nelse return 'a new arrow has a y ref the format does not define';"
          "\nif (toLog(100, true, 'y') !== 2) return 'a data y is not converted onto a log axis';"
          "\nif (toLog(0.75, true, 'paper') !== 0.75) return 'a paper arrow was converted anyway';"
          # and back out again: a drag on a log axis is reported in log10, so the store only
          # stays in data units if unLog undoes exactly what toLog did
          "\nif (Math.abs(unLog(toLog(91.1876, true, 'x'), true, 'x') - 91.1876) > 1e-9)"
          "  return 'a log round trip does not come back to where it started';"
          "\nif (unLog(0.75, true, 'paper') !== 0.75) return 'a paper drag was converted anyway';"
          "\nif (unLog(91.1876, false, 'x') !== 91.1876) return 'a linear drag was converted anyway';"
          "\npd.cmt = 'checked by smoke';"
          "\nvar CC = JSON.stringify(collectState());"
          "\napplyState(JSON.parse(CC));"
          "\nif (JSON.stringify(collectState()) !== CC) return 'notes did not survive the round trip';"
          "\npd = PLOTS[0];"
          "\nif (pd.cmt !== 'checked by smoke' || !pd.draw[dk] || pd.draw[dk].shapes.length !== 1)"
          "  return 'the applied plot lost its notes';"
          "\ndelete pd.draw[dk]; pd.cmt = '';"
          "\npd.mode = wasM; pd.view = wasV; pd.pull = wasP; pd.varx = wasX;"
          "\npd.compare = wasC; pd.anno = wasA;"
          "\ndrawAll();"
          "\nDR = 'ok';"
          "\nvar RO = 'skipped';"
          "\nif (DOC.rohide && DOC.readout){""  if (!ROFOLD) return 'the counts bar boots open';""  if (!DOC.readout.hidden) return 'the counts bar boots shown';""  DOC.rohide.onclick();""  if (ROFOLD || DOC.readout.hidden) return 'the handle did not unfold the counts';""  if (DOC.tabbar.innerHTML.indexOf('rofold') < 0)""    return 'the fold handle does not ride in the tab row';""  if (DOC.tabbar.innerHTML.indexOf('counts') < 0)""    return 'the folded handle lost its label';""  var st = JSON.stringify(collectState());""  if (st.indexOf('ROFOLD') >= 0 || st.indexOf('rofold') >= 0)""    return 'a fold reached the saved state';""  DOC.rohide.onclick();""  if (!ROFOLD || !DOC.readout.hidden) return 'the counts did not fold again';""  RO = 'boots folded';""}"
          # the builder shell: opens over the page, Escape closes exactly once, the ✕
          # cancels, and a closed builder ignores keys. Guarded so pre-redesign pages skip.
          "\nvar BS = 'skipped';"
          "\nif (typeof openBuilder === 'function'){"
          "  openBuilder();"
          "  if (!BLD.open || DOC.bld.hidden) return 'openBuilder did not open the overlay';"
          "  bldKey({key:'x'});"
          "  if (!BLD.open) return 'a non-Escape key closed the builder';"
          "  bldKey({key:'Escape'});"
          "  if (BLD.open || !DOC.bld.hidden) return 'Escape did not close the builder';"
          "  bldKey({key:'Escape'});"
          "  if (BLD.open || !DOC.bld.hidden) return 'Escape on a closed builder reopened it';"
          "  openBuilder(); DOC.bldx.onclick();"
          "  if (BLD.open || !DOC.bld.hidden) return 'the cancel button did not close the builder';"
          # the layout stage: count, drag/swap, commit -> real plots with sheet+quad,
          # rendered as a quadrant canvas, riding through the state; then cleaned up
          "  openBuilder();"
          "  if (!BLD.draft || BLD.draft.quads.length !== 1) return 'a fresh draft is not one plot';"
          # plots start UNFILLED whatever the band has enabled: bare axes until a drop
          "  if (BLD.draft.specs[0].lines === null || BLD.draft.specs[0].lines.length)"
          "    return 'a fresh plot is not empty';"
          "  if (BLD.draft.specs[0].layer) return 'a fresh plot already has a layer';"
          "  bldSetCount(3);"
          "  if (BLD.draft.quads.length !== 3) return 'setCount(3) made ' + BLD.draft.quads.length;"
          "  if (new Set(BLD.draft.quads).size !== 3) return 'setCount packed two cards into one quadrant';"
          "  bldSetCount(2);"
          "  if (BLD.draft.quads.length !== 2) return 'setCount(2) made ' + BLD.draft.quads.length;"
          "  bldMove(0, 3);"
          "  if (BLD.draft.quads[0] !== 3) return 'bldMove did not move the card';"
          "  var occ = BLD.draft.quads[1];"
          "  bldMove(1, 3);"
          "  if (BLD.draft.quads[1] !== 3 || BLD.draft.quads[0] !== occ)"
          "    return 'dropping on an occupied quadrant did not swap';"
          "  var nP = PLOTS.length, sh = BLD.draft.sheet, qs = BLD.draft.quads.slice();"
          "  bldCommit();"
          "  if (BLD.open) return 'commit left the builder open';"
          "  if (PLOTS.length !== nP + 2) return 'commit made ' + (PLOTS.length - nP) + ' plots for 2 cards';"
          "  var made = PLOTS.slice(nP);"
          "  if (made.some(function(p, k){ return p.sheet !== sh || p.quad !== qs[k]; }))"
          "    return 'a committed plot lost its sheet or quadrant';"
          "  if (DOC.plots.innerHTML.indexOf('class=\"sheet\"') < 0"
          "      || DOC.plots.innerHTML.indexOf('data-n=\"2\"') < 0)"
          "    return 'the page did not render the new sheet as a quadrant canvas';"
          "  var stq = collectState(), lastq = stq.plots[stq.plots.length - 1];"
          "  if (lastq.sheet !== sh || lastq.quad !== qs[1]) return 'sheet/quad missing from the state';"
          "  applyState(stq);"
          "  if (PLOTS[PLOTS.length - 1].sheet !== sh) return 'sheet did not survive applyState';"
          "  var planq = sheetPlan(PLOTS);"
          "  var solos = PLOTS.filter(function(p){ return p.sheet == null; }).length;"
          "  var sheets = new Set(PLOTS.filter(function(p){ return p.sheet != null; })"
          "                 .map(function(p){ return p.sheet; })).size;"
          "  if (planq.length !== solos + sheets)"
          "    return 'sheetPlan made ' + planq.length + ' units for ' + solos + ' solos + ' + sheets + ' sheets';"
          "  PLOTS = PLOTS.slice(0, nP);"
          "  renderPlots(); drawAll();"
          # the data stage: sentences drive the specs, the preview draws through the
          # page's own pipeline, and the committed plots carry the sentence
          "  openBuilder(); bldSetCount(2);"
          "  bldGoto('data');"
          "  if (BLD.stage !== 'data') return 'bldGoto did not reach the data stage';"
          # the layout canvas and the sample tray never leave the data stage
          "  if (DOC.bldstage.innerHTML.indexOf('bldquads') < 0)"
          "    return 'the data stage lost the layout canvas';"
          "  if (DOC.bldstage.innerHTML.indexOf('bldtray') < 0)"
          "    return 'the data stage has no sample tray';"
          "  if (DOC.bldtray.innerHTML.indexOf('chip') < 0)"
          "    return 'the tray does not carry the band chips';"
          # the tray is a PICKER: no destructive x's — structure belongs to the band
          "  if (DOC.bldtray.innerHTML.indexOf('data-del') >= 0"
          "      || DOC.bldtray.innerHTML.indexOf('data-subdel') >= 0)"
          "    return 'the tray still carries destructive buttons';"
          # the tray speaks PER-PLOT: an empty plot presses nothing, whatever the band shows
          "  if (DOC.bldtray.innerHTML.indexOf('aria-pressed=\"true\"') >= 0)"
          "    return 'the tray presses chips an empty plot does not hold';"
          # page two renders NO plotly: mode, variables and samples only — the preview
          # and its WebGL live on the style page alone
          "  if (DOC.bldstage.innerHTML.indexOf('plotbld') >= 0)"
          "    return 'the data stage still carries a live preview';"
          "  var c2 = META.cols[1] ? META.cols[1].name : META.cols[0].name;"
          "  bldSpec(1, 'mode', '2d');"
          "  bldSpec(1, 'varx', c2);"
          "  if (BLD.active !== 1) return 'editing a slot did not make it active';"
          # filling by drop: the quadrant takes the chip as its layer, the card shows
          # the membership square with its own x
          "  if (!bldDropSrc(1, bldSources()[0])) return 'the quadrant drop refused a source';"
          "  if (BLD.draft.specs[1].lines.indexOf(bldSources()[0]) < 0)"
          "    return 'the drop did not join the 2D sum';"
          "  if (DOC.bldstage.innerHTML.indexOf('bldmini') < 0)"
          "    return 'the card does not show its membership';"
          "  if (DOC.bldstage.innerHTML.indexOf('data-unmini') < 0)"
          "    return 'the membership square has no x';"
          "  if (DOC.bldstage.innerHTML.indexOf('summed') < 0)"
          "    return 'the 2D sum warning is missing';"
          "  if (DOC.bldtray.innerHTML.indexOf('aria-pressed') >= 0)"
          "    return 'the tray still wears on and off states';"
          # add-only: a duplicate is refused, the x takes it out, a re-pick refills
          "  if (bldDropSrc(1, bldSources()[0])) return 'a duplicate filled the panel twice';"
          "  bldUnpick(1, bldSources()[0]);"
          "  if (BLD.draft.specs[1].lines.length) return 'the x did not empty the panel';"
          "  bldPick(bldSources()[0]);"
          "  if (BLD.draft.specs[1].lines.indexOf(bldSources()[0]) < 0)"
          "    return 'the re-pick did not refill';"
          # NOTHING commits before apply: a hidden name joins the draft, the page is untouched
          "  BLD.active = 0;"
          "  var hid9 = bldSources()[0];"
          "  (groupNames().indexOf(hid9) >= 0 ? members(hid9)"
          "     : [META.datasets.indexOf(hid9)]).forEach(function(j){ S.show[j] = false; });"
          "  drawAll();"
          "  if (!bldDropSrc(0, hid9)) return 'the 1D drop refused a real source';"
          "  if (srcShown(hid9)) return 'the builder touched the page before apply';"
          "  if (BLD.draft.specs[0].lines.indexOf(hid9) < 0) return 'the 1D drop did not add the line';"
          # previews live on the STYLE page alone; a hidden member draws an honest zero
          "  DRAWN = [];"
          "  bldGoto('style');"
          "  if (!DRAWN.length) return 'the draft did not preview at all';"
          "  bldGoto('data');"
          "  bldUnpick(0, hid9);"
          "  if (BLD.draft.specs[0].lines.length) return 'the x did not remove the line';"
          "  bldPick(hid9);"
          # a bundle joins a plot as ONE curve by its own name, when the study has one
          "  var sg9 = null;"
          "  groupNames().forEach(function(g){ if (!sg9 && subNames(g).length) sg9 = subNames(g)[0]; });"
          "  if (sg9){"
          "    if (!bldPick(sg9)) return 'a bundle was refused as a member';"
          "    if (BLD.draft.specs[0].lines.indexOf(sg9) < 0) return 'the bundle did not join the lines';"
          "    bldUnpick(0, sg9);"
          "  }"
          # combining is the BAND's job alone now — the sentence says so
          "  if (DOC.bldstage.innerHTML.indexOf('group them') < 0"
          "      && DOC.bldstage.innerHTML.indexOf('grouping is the one way') < 0)"
          "    return 'the sentence does not point combining at the band';"
          "  BLD.active = 1;"
          "  var nP2 = PLOTS.length;"
          "  bldCommit();"
          "  var m2 = PLOTS[PLOTS.length - 1];"
          "  if (m2.mode !== '2d' || m2.varx !== c2) return 'the sentence did not reach the committed plot';"
          # APPLY is the moment the page changes: the hidden name switched on here
          "  if (!srcShown(hid9)) return 'apply did not switch the named source on';"
          "  PLOTS = PLOTS.slice(0, nP2);"
          "  renderPlots(); drawAll();"
          # the style stage: restrained keys ride the spec into the committed plot;
          # the slot is filled first — an unfilled plot honestly previews nothing
          "  openBuilder();"
          "  if (bldSources().length) bldDropSrc(0, bldSources()[0]);"
          "  bldGoto('style');"
          "  if (BLD.stage !== 'style') return 'bldGoto did not reach the style stage';"
          "  if (DOC.bldstage.innerHTML.indexOf('bldquads') < 0)"
          "    return 'the style stage lost the layout canvas';"
          "  if (DOC.bldstage.innerHTML.indexOf('lbx') < 0)"
          "    return 'the style stage lost the log-binning toggles';"
          "  BLD.draft.specs[0].nb = 25; BLD.draft.specs[0].span = 'full';"
          "  BLD.draft.specs[0].logy = true;"
          "  DRAWN = [];"
          "  bldPreview();"
          "  if (!DRAWN.length) return 'the style preview drew nothing';"
          "  var nP3 = PLOTS.length;"
          "  bldCommit();"
          "  var m3 = PLOTS[PLOTS.length - 1];"
          "  if (m3.nb !== 25 || m3.span !== 'full' || !m3.logy)"
          "    return 'style keys did not reach the committed plot';"
          "  PLOTS = PLOTS.slice(0, nP3);"
          "  renderPlots(); drawAll();"
          # modify: edit mode opens prefilled on data, cancel costs nothing, apply
          # lands in place under the same id without changing the plot count
          "  var pe = PLOTS[0], peid = pe.id, oldnb = pe.nb;"
          "  openBuilderFor(pe);"
          "  if (BLD.mode !== 'edit' || BLD.stage !== 'data')"
          "    return 'modify did not open in edit mode on data';"
          "  if (BLD.draft.specs[BLD.active].varx !== pe.varx)"
          "    return 'the edit draft is not prefilled';"
          "  BLD.draft.specs[BLD.active].nb = 77;"
          "  closeBuilder();"
          "  if (PLOTS[0].nb === 77) return 'cancel leaked a draft edit onto the page';"
          "  openBuilderFor(PLOTS[0]);"
          "  BLD.draft.specs[BLD.active].nb = 77;"
          "  var nPe = PLOTS.length;"
          "  bldCommit();"
          "  if (PLOTS.length !== nPe) return 'apply changed the plot count';"
          "  if (PLOTS[0].nb !== 77 || PLOTS[0].id !== peid)"
          "    return 'apply did not land in place';"
          "  PLOTS[0].nb = oldnb;"
          "  renderPlots(); drawAll();"
          # a 2D slot picks its dataset in the builder: the layer select commits, and a
          # loose sample is as legal a layer as a group (layerSel matches it by name).
          # The pick may auto-enable a hidden source, so visibility is restored after.
          "  var shL = collectState();"
          "  openBuilder(); bldGoto('data');"
          "  bldSpec(0, 'mode', '2d');"
          "  if (DOC.bldstage.innerHTML.indexOf('optgroup') < 0)"
          "    return 'the add dropdown is not organized like the band';"
          "  var srcs2 = bldSources();"
          "  bldPick(srcs2[srcs2.length - 1]);"
          "  var nP5 = PLOTS.length;"
          "  bldCommit();"
          "  var m5 = PLOTS[PLOTS.length - 1];"
          "  if (!m5.lines || m5.lines.indexOf(srcs2[srcs2.length - 1]) < 0)"
          "    return 'the added member did not commit';"
          "  DRAWN = [];"
          "  drawOne(m5, MASK);"
          "  if (!DRAWN.length) return 'the 2D sum drew nothing';"
          "  PLOTS = PLOTS.slice(0, nP5);"
          "  applyState(shL);"
          "  renderPlots(); drawAll();"
          # picking a HIDDEN source as a 2D layer switches it on instead of drawing
          # an empty map — an explicit pick beats a hidden toggle
          "  var sh5 = collectState();"
          "  openBuilder(); bldGoto('data'); bldSpec(0, 'mode', '2d');"
          "  var pick5 = bldSources()[bldSources().length - 1];"
          "  var di5 = META.datasets.indexOf(pick5);"
          "  var mi5 = di5 >= 0 ? [di5] : members(pick5);"
          "  mi5.forEach(function(i){ S.show[i] = false; });"
          "  if (srcShown(pick5)) return 'srcShown missed a hidden source';"
          "  bldPick(pick5);"
          "  if (srcShown(pick5)) return 'the add path touched the page before apply';"
          "  closeBuilder();"
          "  applyState(sh5); renderPlots(); drawAll();"
          # per-plot merge: a local 1D line over the global groups — the spec carries it,
          # mergeOf resolves it, the line draws under its own name, cutText records it,
          # and a merge may never shadow a global group
          "  var MG = groupNames();"
          "  openBuilder();"
          "  BLD.draft.specs[0].lines = null;"   # the merge tests want everything drawing
          "  var srcs = bldSources();"
          "  if (srcs.length){"
          "    if (MG.length && bldMergeAdd(0, MG[0], [srcs[0]]) !== false)"
          "      return 'a merge was allowed to shadow a global group';"
          "    if (!bldMergeAdd(0, 'smoke line', [srcs[0]]))"
          "      return 'bldMergeAdd refused a valid merge';"
          "    if (BLD.draft.specs[0].merge[srcs[0]] !== 'smoke line')"
          "      return 'the merge did not land in the spec';"
          "    if (srcs.length > 1){"
          "      bldMergeAdd(0, 'droppable', [srcs[1]]);"
          "      bldMergeDrop(0, 'droppable');"
          "      if (BLD.draft.specs[0].merge"
          "          && Object.values(BLD.draft.specs[0].merge).indexOf('droppable') >= 0)"
          "        return 'bldMergeDrop left the merged line behind';"
          "    }"
          "    var nP4 = PLOTS.length;"
          "    bldCommit();"
          "    var m4 = PLOTS[PLOTS.length - 1];"
          "    if (!m4.merge || m4.merge[srcs[0]] !== 'smoke line')"
          "      return 'the merge did not reach the plot';"
          "    var di4 = -1;"
          "    META.datasets.forEach(function(d, ii){"
          "      if (di4 < 0 && (S.group[ii] === srcs[0] || (!S.group[ii] && d === srcs[0]))) di4 = ii; });"
          "    if (di4 < 0 || mergeOf(m4, di4) !== 'smoke line')"
          "      return 'mergeOf did not resolve the constituent';"
          "    m4.mode = '1d';"
          "    DRAWN = [];"
          "    drawOne(m4, MASK);"
          "    if (!DRAWN.some(function(t){ return t.name && t.name.indexOf('smoke line') === 0; }))"
          "      return 'the merged line did not draw under its name';"
          "    var stm = collectState();"
          "    if (stm.plots[stm.plots.length - 1].merge[srcs[0]] !== 'smoke line')"
          "      return 'the merge is missing from the state';"
          "    if (cutText().indexOf('merge=') < 0) return 'cutText does not record the merge';"
          # the plot's own line list: only listed sources draw, [] draws nothing,
          # and the choice rides the state and the cut text
          "    m4.lines = [srcs[0]];"
          "    DRAWN = [];"
          "    drawOne(m4, MASK);"
          "    if (!DRAWN.length) return 'the lines filter drew nothing at all';"
          "    if (DRAWN.some(function(t){ return t.name && t.name.indexOf('smoke line') !== 0"
          "        && t.name !== 'pull'; }))"
          "      return 'the lines filter drew an excluded source';"
          "    if (cutText().indexOf('lines=') < 0) return 'cutText does not record the lines';"
          "    m4.lines = [];"
          "    drawOne(m4, MASK);"
          "    if (DRAWN.length) return 'an empty lines list still drew';"
          "    var stl = collectState();"
          "    if (JSON.stringify(stl.plots[stl.plots.length - 1].lines) !== '[]')"
          "      return 'lines is missing from the state';"
          "    PLOTS = PLOTS.slice(0, nP4);"
          "    renderPlots(); drawAll();"
          "  }"
          "  BS = 'ok';"
          "}"
          # Rover: the frame renderer is pure — height, eyes, blink, turning
          "\nvar RVR = 'skipped';"
          "\nif (typeof roverFrame === 'function'){"
          "  var f0 = roverFrame(0, false);"
          "  if (f0.split('\\n').length !== 16) return 'rover is the wrong height';"
          "  if (f0.indexOf('\\u2022') < 0) return 'rover has no eyes at rest';"
          "  if (roverFrame(0, true).indexOf('\\u2022') >= 0) return 'rover did not blink';"
          "  if (roverFrame(0.9, false) === f0) return 'rover does not turn';"
          "  RVR = 'ok';"
          "}"
          # the drawer: toggles the rail, rides the state, applyState reopens it; and
          # every plot carries the bottom rescale bar
          "\nvar DW = 'skipped';"
          "\nif (typeof setDrawer === 'function'){"
          "  setDrawer(true);"
          "  if (DOC.rail.hidden) return 'setDrawer(true) left the rail hidden';"
          "  setDrawer(false);"
          "  if (!DOC.rail.hidden) return 'setDrawer(false) left the rail out';"
          "  if (DOC.plots.innerHTML.indexOf('class=\"pbar\"') < 0)"
          "    return 'plots carry no rescale bar';"
          "  if (DOC.plots.innerHTML.indexOf('cmtbox') < 0)"
          "    return 'plots carry no notes overlay';"
          "  if (DOC.plots.innerHTML.indexOf('cmtgrip') < 0)"
          "    return 'the notes overlay lost its drag grip';"
          # the cuts stamp for exported plots: lines match the active cuts, the
          # annotation exists when cuts do, and the option rides the state
          "  if (cutLines().length !== activeCuts().length)"
          "    return 'cutLines disagrees with activeCuts';"
          "  if (activeCuts().length && !cutsAnno()) return 'cutsAnno made nothing';"
          "  DOC.expcuts.checked = true;"
          "  if (collectState().expcuts !== true) return 'expcuts is missing from the state';"
          "  DOC.expcuts.checked = false;"
          # 1D's log-y is the COUNT axis and must never bend a 2D variable axis
          "  var pl2 = PLOTS[0], keep2 = {mode: pl2.mode, logy: pl2.logy, lby: pl2.lby};"
          "  pl2.mode = '2d'; pl2.logy = true; pl2.lby = false;"
          "  drawOne(pl2, MASK);"
          "  if (LAY.yaxis.type === 'log') return \"1D's log-y bent a 2D variable axis\";"
          "  pl2.mode = keep2.mode; pl2.logy = keep2.logy; pl2.lby = keep2.lby;"
          "  drawOne(pl2, MASK);"
          # the drawer and its folds are page chrome: tab clicks must not move them
          "  setDrawer(true);"
          "  var cntWas = DOC.cntbox.open;"
          "  DOC.cntbox.open = false;"
          "  var tCh = TAB;"
          "  newTab(false);"
          "  if (DOC.rail.hidden) return 'a new tab shut the drawer';"
          "  if (DOC.cntbox.open) return 'a new tab refolded a rail section';"
          "  switchTab(tCh);"
          "  if (DOC.rail.hidden) return 'switching back shut the drawer';"
          "  closeTab(TABS.length - 1);"
          "  DOC.cntbox.open = cntWas;"
          "  setDrawer(false);"
          "  DW = 'ok';"
          "}"
          # locks ship OFF; locked cuts/plots follow a tab switch; the nukes strip the
          # tab and the file survives; dirty flips on redraw and clears on demand
          "\nvar LK = 'skipped';"
          "\nif (typeof nukeCuts === 'function'){"
          "  if (LOCK.cuts || LOCK.plots || LOCK.show) return 'locks must ship off';"
          "  var keepAll = collectState();"
          "  var tab9 = TAB, c9 = activeCuts().length;"
          "  newTab(false);"
          "  switchTab(tab9);"
          # compare what the lock owns (which plots) — a group-less factory tab honestly
          # sanitizes a carried layer to 'all', so full-JSON equality would be wrong
          "  var psig = function(ps){ return JSON.stringify(ps.map(function(p){"
          "    return [p.mode, p.varx, p.nb]; })); };"
          "  var cutsA = JSON.stringify(pickCuts(collectState(), function(){ return true; }));"
          "  var plotsA = psig(collectState().plots);"
          "  LOCK.cuts = true; LOCK.plots = true;"
          "  switchTab(TABS.length - 1);"
          "  if (JSON.stringify(pickCuts(collectState(), function(){ return true; })) !== cutsA)"
          "    return 'locked cuts did not follow the tab switch';"
          "  if (psig(collectState().plots) !== plotsA)"
          "    return 'locked plots did not follow the tab switch';"
          "  var tm9 = tunedMeta();"
          "  if (tm9.lockcuts !== true || tm9.lockplots !== true)"
          "    return 'tunedMeta forgot the locks';"
          "  LOCK.cuts = false; LOCK.plots = false;"
          "  switchTab(tab9);"
          "  closeTab(TABS.length - 1);"
          "  nukeCuts();"
          "  if (activeCuts().length !== 0) return 'nukeCuts left ' + activeCuts().length + ' cuts';"
          "  nukePlots();"
          "  if (PLOTS.length !== 0) return 'nukePlots left plots';"
          "  drawAll();"
          "  applyState(keepAll);"
          "  renderPlots(); drawAll();"
          "  if (activeCuts().length !== c9) return 'restore after the nukes failed';"
          # setLock: ticking makes THIS tab the shared copy and stashes every tab's own;
          # unticking hands each tab its own configuration back
          "  var openCuts;"
          "  newTab(false);"
          "  openCuts = activeCuts().length;"
          "  switchTab(tab9);"
          "  setLock('cuts', true);"
          "  if (DOC.lockcutstxt.textContent.indexOf('synced') < 0)"
          "    return 'the ticked label does not say synced';"
          "  switchTab(TABS.length - 1);"
          "  if (activeCuts().length !== c9) return 'setLock did not share the cuts';"
          "  setLock('cuts', false);"
          "  if (DOC.lockcutstxt.textContent.indexOf('share this') < 0)"
          "    return 'the unticked label does not offer the share';"
          "  if (activeCuts().length !== openCuts)"
          "    return 'unlock did not give the tab its own cuts back';"
          "  if (LOCK.cuts) return 'unlock left the flag on';"
          "  switchTab(tab9);"
          "  closeTab(TABS.length - 1);"
          # the samples lock: ticking shares which samples are lit; unticking hands
          # each tab its own show map back
          "  newTab(false);"
          "  S.show[0] = !S.show[0];"
          "  var own8 = JSON.stringify(collectState().show);"
          "  switchTab(tab9);"
          "  var home8 = JSON.stringify(collectState().show);"
          "  setLock('show', true);"
          "  switchTab(TABS.length - 1);"
          "  if (JSON.stringify(collectState().show) !== home8)"
          "    return 'the show lock did not share the samples';"
          "  var tm8 = tunedMeta();"
          "  if (tm8.lockshow !== true) return 'tunedMeta forgot the show lock';"
          "  setLock('show', false);"
          "  if (JSON.stringify(collectState().show) !== own8)"
          "    return 'unlock did not give the tab its own samples back';"
          "  if (LOCK.show) return 'show unlock left the flag on';"
          "  switchTab(tab9);"
          "  closeTab(TABS.length - 1);"
          "  clearDirty();"
          "  if (DIRTY) return 'clearDirty did not clear';"
          "  drawOne(PLOTS[0], MASK);"
          "  if (!DIRTY) return 'a redraw did not mark work unsaved';"
          "  LK = 'ok';"
          "}"
          # Tables: a board is something a TAB HOLDS, not a kind of tab — there is no
          # newTable() any more, only addTable() on a tab you are already on. Yields,
          # metrics, minis and the Σ shelf are checked against straight loops and the
          # optimizer's own formulas; the table rides the state; leaving and closing put
          # everything back.
          "\nvar TM = 'skipped';"
          "\nif (typeof addTable === 'function'){"
          "  var tabs9 = TABS.length, tabHome = TAB;"
          "  var keepShow9 = JSON.stringify(collectState().show);"
          "  newTab(false);"
          "  addTable();"
          "  if (!TBL) return 'addTable did not put a board on the tab';"
          # adding twice must not silently replace the board someone has been filling in
          "  var board9 = TBL;"
          "  addTable();"
          "  if (TBL !== board9) return 'a second addTable replaced the board';"
          # and it has to come back off again, taking nothing else with it
          "  PLOTS.push(newPlot({nb: 11})); renderPlots(); drawAll();"
          "  var plots9 = PLOTS.length;"
          "  dropTable();"
          "  if (TBL) return 'dropTable left the board on the tab';"
          "  if (DOC.plots.innerHTML.indexOf('tblboard') >= 0) return 'the board outlived dropTable';"
          "  if (PLOTS.length !== plots9) return 'dropTable took the plots with it';"
          "  PLOTS.pop();"
          "  addTable(); renderPlots(); drawAll();"
          "  if (!TBL) return 'a table could not be added back after dropTable';"
          "  if (TABS.length !== tabs9 + 1) return 'the table probe made ' + (TABS.length - tabs9) + ' tabs';"
          # a board copies whole and lands on a tab that has none — and REFUSES one that
          # already does, since replacing it would destroy a board someone built
          "  if (typeof copyTable === 'function'){"
          "    TBL.rows = 3; TBL.cols = 3; TBL.err = 'sumw2';"
          "    copyTable();"
          "    if (!CLIP || CLIP.kind !== 'table') return 'copyTable put no table on the clipboard';"
          "    var held9 = TBL;"
          "    pasteBlock();"
          "    if (TBL !== held9) return 'pasting onto a tab that has a table replaced it';"
          # a REFUSED paste pasted nothing, so it must not spend the clipboard either
          "    if (!CLIP) return 'a refused paste spent the clipboard';"
          "    newTab(false);"
          "    if (TBL) return 'a fresh tab inherited a table';"
          # hold the clipboard's own object: the paste is about to spend CLIP, and the
          # aliasing checks below still need something to compare the pasted board against
          "    var ctbl9 = CLIP.tbl;"
          "    pasteBlock();"
          "    if (!TBL) return 'the table did not paste into a tab that had none';"
          "    if (CLIP) return 'the table paste did not spend the clipboard';"
          "    if (TBL.rows !== 3 || TBL.err !== 'sumw2') return 'the pasted table lost its setup';"
          "    if (TBL === ctbl9) return 'the pasted table aliases the clipboard';"
          "    TBL.rows = 4;"
          "    if (ctbl9.rows !== 3) return 'editing the pasted table reached the clipboard';"
          "    closeTab(TAB); switchTab(TABS.length - 1);"
          "    CLIP = null; pasteBtn();"
          "    TBL.rows = 2; TBL.cols = 2; TBL.err = 'poisson_neff';"
          "    renderPlots(); drawAll();"
          "  }"
          "  if (TBL.rows !== 2 || TBL.cols !== 2 || TBL.cells.length !== 4)"
          "    return 'a fresh board is not 2x2';"
          "  if (DOC.plots.innerHTML.indexOf('tblboard') < 0) return 'the board did not render';"
          # the counts bar has no business here — the board IS the counts, and the handle
          # goes with it rather than offering to unfold something that would not come back
          "  if (!DOC.readout.hidden) return 'the counts bar crowds a tab holding a table';"
          "  if (!DOC.rohide.hidden) return 'the fold handle outlived the bar it folds';"
          # and the FOM pair says so rather than looking live
          "  if (DOC.fomnwarn.hidden || DOC.fomdwarn.hidden)"
          "    return 'the FOM rail does not warn that it is inert on a table';"
          "  var ts9 = bldSources();"
          "  if (ts9.length){"
          "    tblAssign(0, ts9[0]);"
          "    if (TBL.cells[0] !== ts9[0]) return 'assign did not land in the cell';"
          "    var tb9 = ts9[ts9.length - 1];"
          "    tblBkg(0, tb9);"
          "    if (TBL.bkgs.length !== 1) return 'the shelf did not take a background';"
          "    tblBkg(1, tb9);"
          "    if (TBL.bkgs.length !== 1) return 'the shelf accepted a duplicate';"
          # Σ background adds the shelf slot by slot, so a source nested inside another
          # would be counted twice: the outer one REPLACES what it contains, and a member
          # of something already there is refused. Measured, not asserted from the names.
          "    var subs9 = [];"
          "    META.datasets.forEach(function(d, i){"
          "      if (S.sub[i] && subs9.indexOf(S.sub[i]) < 0) subs9.push(S.sub[i]); });"
          "    var out9 = null, in9 = null;"
          "    groupNames().forEach(function(g){"
          "      if (out9) return;"
          "      var big = tblIdx(g);"
          "      subs9.forEach(function(s){"
          "        if (out9) return;"
          "        var sm = tblIdx(s);"
          "        if (sm.length && sm.length < big.length"
          "            && sm.every(function(i){ return big.indexOf(i) >= 0; })){"
          "          out9 = g; in9 = s; } }); });"
          "    var NEST = 'no nesting in this study';"
          "    if (out9){"
          "      TBL.bkgs = []; tblBkg(0, out9); drawTable(MASK);"
          "      var alone9 = TBLY.bn;"
          "      tblBkg(1, in9); drawTable(MASK);"
          "      if (TBL.bkgs.length !== 1 || TBL.bkgs[0] !== out9)"
          "        return 'the shelf took ' + in9 + ' while ' + out9 + ' already contains it';"
          "      if (TBLY.bn !== alone9)"
          "        return '\\u03a3 background moved when a nested source was refused: '"
          "               + TBLY.bn + ' vs ' + alone9;"
          # and the other direction: the outer group swallows the member already there
          "      TBL.bkgs = []; tblBkg(0, in9); drawTable(MASK);"
          "      var inn9 = TBLY.bn;"
          "      tblBkg(1, out9); drawTable(MASK);"
          "      if (TBL.bkgs.length !== 1 || TBL.bkgs[0] !== out9)"
          "        return 'the outer group did not replace the member it contains';"
          "      if (TBLY.bn !== alone9)"
          "        return 'the replaced shelf does not equal the group alone: '"
          "               + TBLY.bn + ' vs ' + alone9;"
          "      if (alone9 === inn9) return 'the nesting probe picked two equal sources';"
          "      NEST = out9 + ' swallows ' + in9 + ' (' + alone9 + ' not ' + (alone9 + inn9) + ')';"
          "      TBL.bkgs = []; tblBkg(0, tb9);"
          "    }"
          # a filled tile drags onto another and the two trade squares
          "    tblAssign(0, ts9[0]); tblAssign(3, null);"
          "    tblSwapCells('cell:0', 3);"
          "    if (TBL.cells[3] !== ts9[0] || TBL.cells[0]) return 'the tile did not move square';"
          "    tblAssign(0, tb9);"
          "    tblSwapCells('cell:3', 0);"
          "    if (TBL.cells[0] !== ts9[0] || TBL.cells[3] !== tb9)"
          "      return 'two filled tiles did not swap';"
          "    tblAssign(3, null);"
          "    drawTable(MASK);"
          "    var want9 = {}; tblIdx(ts9[0]).forEach(function(i){ want9[i] = 1; });"
          # re-derived from META, NOT by calling the page's WTD(): a check that asks the
          # code under test whether it should weight is not an independent check
          "    var wtd9 = (('weights' in META) ? META.weights : 'weight') && S.wt === 'w';"
          "    var sw9 = 0, sn9 = 0;"
          "    for (var r9 = 0; r9 < META.n; r9++)"
          "      if (MASK[r9] && want9[D.__ds[r9]]){"
          "        sw9 += (wtd9 ? D.__w[r9]*LUMI() : 1); sn9++; }"
          "    var a9 = TBLY.acc[ts9[0]];"
          "    if (!a9 || Math.abs(a9.w - sw9) > 1e-9*(1 + Math.abs(sw9)) || a9.n !== sn9)"
          "      return 'the cell yield disagrees with a straight loop: '"
          "             + (a9 && a9.w) + ' vs ' + sw9;"
          "    if (DOC.tblboard.innerHTML.indexOf(ts9[0]) < 0) return 'the cell does not wear its title';"
          "    if (DOC.tblshelf.innerHTML.indexOf('background') < 0) return 'no \\u03a3 background cell';"
          "    if (DOC.tblboard.innerHTML.indexOf('optgroup') < 0)"
          "      return 'the cell dropdown is not organized like the band';"
          # a single sample sits in a cell even while it lives inside a group —
          # the WZto3LNu lesson: the cell names it, tblIdx counts exactly it
          "    var gi9 = -1;"
          "    META.datasets.forEach(function(d, i){ if (gi9 < 0 && S.group[i]) gi9 = i; });"
          "    if (gi9 >= 0){"
          "      tblAssign(1, META.datasets[gi9]);"
          "      if (tblIdx(META.datasets[gi9]).length !== 1"
          "          || tblIdx(META.datasets[gi9])[0] !== gi9)"
          "        return 'a grouped sample does not resolve to exactly itself';"
          "      if (!TBLY.acc[META.datasets[gi9]])"
          "        return 'a grouped sample could not sit in a cell';"
          "      tblAssign(1, null);"
          "    }"
          "    TBL.channel = 'metrics'; drawTable(MASK);"
          "    var m9 = TBLY.met[ts9[0]];"
          "    var z9 = zbi(a9.w, TBLY.b, TBLY.b2, +S.syst || 0);"
          "    if (isFinite(z9) !== isFinite(m9.zbi) || (isFinite(z9) && Math.abs(z9 - m9.zbi) > 1e-12))"
          "      return 'the board ZBi is not the optimizer ZBi';"
          "    if (TBLY.b > 0 && Math.abs(m9.sob - a9.w/TBLY.b) > 1e-12) return 'S/B is not S over B';"
          "    if (DOC.tblboard.innerHTML.indexOf('tmet') < 0) return 'metrics did not render';"
          # a NEW board opens on S/B alone, and nothing else is in the box with it
          "    if (JSON.stringify(tblMets()) !== '[\"sob\"]')"
          "      return 'a fresh board does not open on S/B alone: ' + JSON.stringify(tblMets());"
          "    if (DOC.tblboard.innerHTML.indexOf('S/B') < 0) return 'S/B is not on the board';"
          "    if (DOC.tblboard.innerHTML.indexOf('ZBi') >= 0)"
          "      return 'a one-figure board is still drawing ZBi';"
          # slots grow and shrink, and each one cycles through what the others are not on
          "    tblSlots(3);"
          "    if (tblMets().length !== 3) return 'three slots did not open';"
          "    if (DOC.tblboard.innerHTML.indexOf('ZBi') < 0) return 'the third slot is empty';"
          # the dial holds the slot buttons and drawTable does NOT rebuild it, so the
          # controls must follow the state rather than sit on the previous one
          "    if ((DOC.plots.innerHTML.match(/tblcyc/g) || []).length !== 3)"
          "      return 'the dial shows ' + (DOC.plots.innerHTML.match(/tblcyc/g) || []).length"
          "           + ' slot rows for 3 slots';"
          # with all three up there is nothing to cycle to, so the affordance goes and
          # the rows stay — they are what a reorder drags
          "    if (DOC.plots.innerHTML.indexOf('data-cyc') >= 0)"
          "      return 'a full board still offers a cycle';"
          "    if ((DOC.plots.innerHTML.match(/data-slot/g) || []).length !== 3)"
          "      return 'the full board lost the rows a reorder drags';"
          # and the order the dial shows is the order the cell draws
          "    var ord9 = tblMets().slice();"
          "    tblSwapSlots(0, 2);"
          "    if (tblMets()[0] !== ord9[2] || tblMets()[2] !== ord9[0])"
          "      return 'dragging a slot did not reorder the box';"
          "    if (tblMets().length !== 3) return 'a reorder changed how many FOMs are up';"
          "    tblSwapSlots(0, 2);"
          "    if (JSON.stringify(tblMets()) !== JSON.stringify(ord9))"
          "      return 'reordering back did not restore the order';"
          "    var pre9 = tblMets()[0];"
          "    tblCycle(0);"
          "    if (tblMets()[0] !== pre9) return 'a full board cycled a slot into a duplicate';"
          "    tblSlots(1); tblCycle(0);"
          "    if (tblMets()[0] === 'sob') return 'the lone slot did not cycle';"
          "    var seen9 = {}; tblMets().forEach(function(k){ seen9[k] = 1; });"
          "    tblCycle(0); tblCycle(0);"
          "    if (tblMets()[0] !== 'sob') return 'cycling three times did not come home';"
          "    tblSlots(2);"
          "    if (tblMets().length !== 2 || tblMets()[0] !== 'sob')"
          "      return 'growing to two lost the first slot';"
          "    if (tblMets()[0] === tblMets()[1]) return 'two slots hold the same figure';"
          "    if ((DOC.plots.innerHTML.match(/tblcyc/g) || []).length !== 2)"
          "      return 'the dial did not follow the slot count down to two';"
          # two FOMs keep BOTH: a click cycles, a drag reorders
          "    if ((DOC.plots.innerHTML.match(/data-cyc/g) || []).length !== 2)"
          "      return 'two slots lost the cycle';"
          "    if ((DOC.plots.innerHTML.match(/data-slot/g) || []).length !== 2)"
          "      return 'two slots cannot be reordered';"
          "    var two9 = tblMets().slice();"
          "    tblSwapSlots(0, 1);"
          "    if (tblMets()[0] !== two9[1] || tblMets()[1] !== two9[0])"
          "      return 'two slots did not swap';"
          "    tblSwapSlots(0, 1);"
          # an OLD file carries no met key at all, and must still show what it showed
          "    var keepm9 = TBL.met; delete TBL.met;"
          "    if (JSON.stringify(tblMets()) !== '[\"sob\",\"sig\",\"zbi\"]')"
          "      return 'a table with no met key stopped showing all three';"
          "    TBL.met = keepm9;"
          "    tblSlots(3); drawTable(MASK);"
          # A table tab HAS a plot column now — the board is the stage and the plots
          # stand under it. The builder fills that column here as on any other tab,
          # and must NOT quietly make a channel instead; channels are the dial's "+". The
          # old rule was the reverse, and pinning it is what caught this change.
          "  if (typeof bldCommit === 'function' && typeof openBuilder === 'function'){"
          "    var pl9 = PLOTS.length, tp9 = TBL.plots.length, ch9 = TBL.channel;"
          "    openBuilder();"
          "    if (!BLD.draft) return 'the builder opened without a draft on a table tab';"
          "    BLD.draft.specs = [{mode:'1d', varx: META.cols[0].name, nb: 17}];"
          "    BLD.draft.quads = [0];"
          "    bldCommit();"
          "    if (PLOTS.length !== pl9 + 1) return 'the builder built nothing on a table tab';"
          "    if (TBL.plots.length !== tp9) return 'the builder made a channel, not a plot';"
          "    if (TBL.channel !== ch9) return 'the builder moved the board off its channel';"
          "    var np9 = PLOTS[PLOTS.length - 1];"
          "    if (np9.nb !== 17) return 'the built spec did not survive';"
          # both stages must be on the page at once: the board did not survive being
          # followed by plots, and the plot is not in an invisible column
          "    if (DOC.plots.innerHTML.indexOf('tblboard') < 0)"
          "      return 'the plot column replaced the board';"
          "    if (DOC.plots.innerHTML.indexOf('id=\"plot' + np9.id + '\"') < 0)"
          "      return 'render put the plot in the invisible plot column';"
          "    drawAll();"
          "    PLOTS.pop(); renderPlots(); drawAll();"
          "  }"
          "    tblAddPlot();"
          "    if (TBL.channel !== 0) return 'adding a plot did not tune to it';"
          "    if (!TBL.plots[0].logy || !TBL.plots[0].logz)"
          "      return 'a fresh table plot is not log by default';"
          "    drawTable(MASK);"
          "    if (!TBLY.hists) return 'the plot channel made no histograms';"
          "    var h9 = TBLY.hists.out[ts9[0]];"
          # an integer column gets one bin per value, which overrides nb — the same rule
          # the page draws by, so the expectation has to be computed the same way
          "    var ib9 = intBins(TBL.plots[0].varx, true);"
          "    var wb9 = ib9 ? ib9.nb : TBL.plots[0].nb;"
          "    if (!h9 || h9.length !== wb9) return 'the mini has the wrong bins';"
          "    var t9 = TBLY.hists.tot, o9 = TBLY.hists.out[tb9];"
          "    for (var q9 = 0; q9 < t9.length; q9++)"
          "      if (Math.abs(t9[q9] - o9[q9]) > 1e-9) return 'the \\u03a3 mini is not the shelf summed';"
          "    if (DOC.tblboard.innerHTML.indexOf('tmini') < 0) return 'no mini was rendered';"
          # a mode flip renders once against outgoing histograms and must not die there
          "    TBL.plots[0].mode = '2d';"
          "    renderPlots();"
          "    drawTable(MASK);"
          "    if (!TBLY.hists.ax.y) return 'the 2D flip did not recompute the histograms';"
          "    TBL.plots[0].mode = '1d';"
          "    drawTable(MASK);"
          "    DOC.tblovl.hidden = true;"
          "    tblExpand(ts9[0], false);"
          "    if (DOC.tblovl.hidden) return 'expand did not open the overlay';"
          "    if (!DRAWN.length) return 'the expanded cell drew nothing';"
          "    tblCollapse();"
          "    if (!DOC.tblovl.hidden) return 'collapse left the overlay open';"
          "    if (TBLX) return 'collapse left the cell marked expanded';"
          "    var T9 = JSON.stringify(collectState().table);"
          "    if (T9 === 'null') return 'the table is missing from the state';"
          "    applyState(JSON.parse(JSON.stringify(collectState())));"
          "    if (JSON.stringify(TBL) !== T9) return 'the table did not survive collect/apply';"
          "    tblSize(3, 3);"
          "    if (TBL.cells.length !== 9 || TBL.cells[0] !== ts9[0]) return 'resize lost the corner cell';"
          "    tblSize(2, 2);"
          "    if (TBL.cells[0] !== ts9[0]) return 'shrinking lost the corner cell';"
          # a row or column comes off one at a time too, and the corner survives
          "    tblSize(TBL.rows, TBL.cols + 1);"
          "    if (TBL.cols !== 3) return 'the column grow did not grow';"
          "    tblSize(TBL.rows, TBL.cols - 1);"
          "    if (TBL.cols !== 2 || TBL.cells[0] !== ts9[0]) return 'dropping a column lost the corner';"
          "    tblSize(TBL.rows - 1, TBL.cols);"
          "    if (TBL.rows !== 1 || TBL.cells[0] !== ts9[0]) return 'dropping a row lost the corner';"
          "    tblSize(2, 2);"
          "    tblDropPlot(0);"
          "    if (TBL.channel !== 'yield' || TBL.plots.length)"
          "      return 'dropping the plot left channel ' + TBL.channel;"
          "  }"
          "  newTab(false);"
          "  if (TBL) return 'a fresh tab inherited the table';"
          "  closeTab(TAB);"
          "  switchTab(TABS.length - 1);"
          "  if (!TBL) return 'the tab holding the table lost its board on the way back';"
          "  closeTab(TAB);"
          "  switchTab(tabHome);"
          "  if (TBL) return 'closing the tab that held the table left TBL set';"
          "  DOC.rohide.onclick();""  if (DOC.readout.hidden) return 'the counts bar did not come back off the table';""  DOC.rohide.onclick();"
          "  if (!DOC.fomnwarn.hidden || !DOC.fomdwarn.hidden)"
          "    return 'the FOM warning stayed up after the table went';"
          "  if (JSON.stringify(collectState().show) !== keepShow9)"
          "    return 'the table probe changed which samples are shown';"
          "  TM = 'ok';"
          "}"
          # the band's chips drop onto plots: 1D adds the line, 2D lands on the panel
          # under the cursor, a grouped sample is refused
          "\nvar PD = 'skipped';"
          "\nif (typeof plotDropSrc === 'function' && bldSources().length){"
          "  var pd9 = PLOTS[0], keepM9 = pd9.mode, keepL9 = pd9.lines ? pd9.lines.slice() : null,"
          "      keepA9 = pd9.layer, keepB9 = pd9.layer2, keepV9 = pd9.view;"
          "  var s9 = bldSources()[0];"
          "  pd9.mode = '1d'; pd9.lines = [];"
          "  if (!plotDropSrc(pd9, s9, 0.2)) return 'a 1D drop refused a real source';"
          "  if (pd9.lines.indexOf(s9) < 0) return 'the 1D drop did not add the line';"
          "  pd9.mode = '2d'; pd9.view = 'two';"
          "  plotDropSrc(pd9, s9, 0.2);"
          "  if (pd9.layer !== s9) return 'a left drop did not land on layer A';"
          "  plotDropSrc(pd9, s9, 0.8);"
          "  if (pd9.layer2 !== s9) return 'a right drop did not land on layer B';"
          # a grouped sample is a legal member on the LIVE page too: its own curve
          "  var gi8 = -1; META.datasets.forEach(function(d, i){ if (gi8 < 0 && S.group[i]) gi8 = i; });"
          "  if (gi8 >= 0){"
          "    pd9.mode = '1d'; pd9.lines = [];"
          "    if (!plotDropSrc(pd9, META.datasets[gi8], 0.5))"
          "      return 'a grouped sample was refused by the live drop';"
          "    DRAWN = []; drawOne(pd9, MASK);"
          "    if (!DRAWN.some(function(t){ return t.name && t.name.indexOf(META.datasets[gi8]) === 0; }))"
          "      return 'the grouped sample did not draw its own curve';"
          "  }"
          # a single-view 2D drop joins the SUM
          "  pd9.mode = '2d'; pd9.view = 'one'; pd9.lines = null; pd9.layer = '';"
          "  plotDropSrc(pd9, s9, 0.3);"
          "  if (!pd9.lines || pd9.lines.indexOf(s9) < 0)"
          "    return 'a single-view 2D drop did not join the sum';"
          "  pd9.mode = keepM9; pd9.lines = keepL9; pd9.layer = keepA9; pd9.layer2 = keepB9;"
          "  pd9.view = keepV9; drawAll();"
          "  PD = 'ok';"
          "}"
          + probe +
          "\nif (LOG.length) return 'MISSING IDS: ' + Array.from(new Set(LOG)).join(', ');"
          "\nreturn 'ALL RAN — plots=' + PLOTS.length + ' events=' + META.n +"
          "       ' shown=' + totals(mask()).kept + ' cutText=' + t.split('\\n').length + ' lines' +"
          "       ' roundtrip=ok groups=' + groupNames().length + ' partial=' + PARTIAL +"
          "       ' move=' + MOVE + ' newgroup=' + NEWG +"
          "       ' cutflow=' + cf.steps.length + 'steps' + ' reorder=' + CO + ' 3d=' + D3 +"
          "       ' colour=' + COL + ' cutsec=' + SEC + ' edits=' + ED + ' bar=' + BAR + ' hat=' + HAT + ' stack=' + STK +"
          "       ' pull=' + PULL + ' pullrange=' + PR + ' shape=' + NORM + ' everything=' + EV.length + 'files' +"
          "       ' panels=' + PAN + ' tabs=' + TB + ' draw=' + DR + ' builder=' + BS +"
          "       ' drawer=' + DW + ' rover=' + RVR + ' locks=' + LK + ' table=' + TM +"
          "       ' nest=' + (typeof NEST === 'string' ? NEST : 'skipped') + ' note=' + NOTE +"
          "       ' notes=' + CMT + ' copyblock=' + CP + ' colours=' + CLR +"
          "       ' folds=' + FOLDS + ' countsfold=' + RO +"
          "       ' size=' + SZ +"
          "       ' plotdrop=' + PD +"
          "       ' view=' + (FILE ? 'from file' : 'factory') +"
          "       ' src=' + DOC.srcname.textContent;\n})();")
    p = "/tmp/smoke.js"; pathlib.Path(p).write_text(js)
    r = subprocess.run(["osascript", "-l", "JavaScript", p], capture_output=True, text=True)
    return (r.stdout or r.stderr).strip()

def roundtrip(name: str) -> str:
    """Write a .rvr carrying a view, read it back, and compare the view and every column."""
    import numpy as np
    import cart, load, page
    columns = load.load(name)
    # the cart says whether these events carry weights; a data skim has none
    wcol = cart.Cart.load(name).weights
    names = list(columns)
    first, second = names[0], names[min(1, len(names) - 1)]   # a study may have one sample
    want = {"v": 2, "cuts": {"0": [1.5, 9.5]}, "chips": {}, "show": {"0": False},
            "group": {first: "A", second: "A"}, "fomn": "A", "fomd": "A",
            "show": {first: False}, "lumi": 137, "scale": 6,
            # cmt AND cmton: the words and whether they are showing both belong to how the
            # plot is meant to be presented, so both must survive the file
            "plots": [{"nb": 7, "cmt": "a stored comment", "cmton": False}]}
    out = page.data(columns, name + "_roundtrip", weights=wcol,
                    tabs=[{"name": "tab 1", "state": want}], dest=load.STUDIES / name)
    meta, back = page.read(out["file"])

    if meta.get("tabs") != [{"name": "tab 1", "state": want}] or meta.get("tab") != 0:
        return f"view changed on the way through: {meta.get('tabs')}"
    for column in next(iter(columns.values())):
        mine = np.concatenate([d[column] for d in columns.values()])
        if mine.dtype == bool:
            mine = mine.astype(np.int32)
        if not np.array_equal(mine, back[column]):
            bad = int((mine != back[column]).sum())
            return f"{column}: {bad} of {len(mine)} values differ after the round trip"
    # extract must recover the setup that was written — the tune-export-extract loop
    lf = page.extract(out["file"])
    tab = lf["view"][0]
    if (tab.get("lumi") != 137 or tab.get("fom") != ("A", "A") or "A" not in lf["groups"]
            or [1.5, 9.5] not in [list(v) for v in tab["cuts"].values()]
            or tab["plots"][0].get("nb") != 7
            or tab["plots"][0].get("cmt") != "a stored comment"
            or tab["plots"][0].get("cmton") is not False):
        return f"extract disagrees with what was written: {lf}"
    pathlib.Path(out["file"]).unlink()
    return (f"state and all {len(back)} columns survived — {meta['n']:,} events, "
            f"{out['mb']:.1f} MB")


#: Enter the table tab the FILE carries and report what the board read out of it, so the
#: pins below can be compared in Python. A board is the stage instead of the plot column,
#: which is why such a tab cannot be the boot tab the rest of the battery probes.
TABLE_PROBE = (
    "\nvar gt9 = -1;"
    "\nfor (var i9 = 0; i9 < TABS.length; i9++) if (TABS[i9].st && TABS[i9].st.table) gt9 = i9;"
    "\nif (gt9 < 0) return 'the file no longer carries a table tab';"
    "\nswitchTab(gt9);"
    "\nif (!TBL) return 'the file table tab did not come up as a board';"
    "\ndrawTable(MASK);"
    "\nif (!TBLY) return 'the board computed no yields';"
    "\nvar bd9 = DOC.tblboard.innerHTML;"
    "\nif (bd9.indexOf('ZBi') < 0) return 'the metrics channel did not render';"
    "\nvar cn9 = {};"
    "\nTBL.cells.filter(Boolean).forEach(function(s){"
    "  if (!TBLY.acc[s]) return; cn9[s] = TBLY.acc[s].n; });"
    "\nreturn 'TABLE OK ' + JSON.stringify({rows:TBL.rows, cols:TBL.cols, cells:TBL.cells,"
    "  bkgs:TBL.bkgs, channel:TBL.channel, err:TBL.err, plots:TBL.plots.length,"
    "  met:tblMets(), cellN:cn9, shelfN:TBLY.bn});"
)

#: A cart holding both real data and weighted MC. Counted through the page's OWN cutflow,
#: at two luminosities, because the property being pinned is that one of them does not move:
#: a sample declaring no weights is never multiplied by a luminosity, whatever its group says.
MIXED_PROBE = (
    "\nS.wt = 'w'; S.lumi = 10; lumiTable();"
    "\nvar c9 = cutflow(), a9 = c9.rows[0];"
    "\nS.lumi = 400; lumiTable();"
    "\nvar d9 = cutflow(), b9 = d9.rows[0];"
    "\nvar lum9 = META.datasets.map(function(d, i){ return LUMI(i); });"
    #: and the badge: silent while data and MC sit in DIFFERENT groups, loud the moment one
    #: group holds both, silent again in raw mode where nothing is scaled at all
    "\nvar apart9 = mixedNote();"
    "\nS.group[0] = 'both'; S.group[1] = 'both';"
    "\nvar both9 = mixedNote();"
    "\ndrawAll();"
    "\nvar plot9 = NOTES[PLOTS[0].id];"
    "\nS.wt = 'raw';"
    "\nvar raw9 = mixedNote();"
    #: and what the PAGE would write back out: its own version, with the stamp still on it
    "\nvar out9 = tunedMeta();"
    "\nreturn 'MIXED OK ' + JSON.stringify({unw: UNW, sumw: Array.from(SUMWDS),"
    "  lumis: lum9,"
    "  groups: c9.gs, raw: a9.raw, at10: a9.by, at400: b9.by,"
    "  apart: apart9, both: both9, rawmode: raw9,"
    "  saves: out9.rowver, keepsStamp: !!(out9.cart && out9.cart.samples),"
    "  onplot: plot9.indexOf(both9) === 0 && plot9.length > both9.length});"
)


#: Enter the tab the FILE carries a partition on and report what the MAP itself computed,
#: so the counts pinned below are checked against the page's own arithmetic as well as
#: against numpy's. The leaves come back in tree order — the predicate side before its
#: complement — which is the order the pins are written in.
REGION_PROBE = (
    "\nvar gr9 = -1;"
    "\nfor (var i9 = 0; i9 < TABS.length; i9++)"
    "  if (TABS[i9].st && TABS[i9].st.regions) gr9 = i9;"
    "\nif (gr9 < 0) return 'the file no longer carries a region map tab';"
    "\nswitchTab(gr9);"
    "\nif (!RGN) return 'the map tab did not come up as a region map';"
    "\ntry { drawAll(); } catch(e){ return 'the map THREW: ' + e.message; }"
    "\nif (!RGNT) return 'the map built no tree';"
    "\nvar lv9 = [];"
    "\n(function rec(n){ if (!n.kids.length){ lv9.push(rgnN(n)); return; } n.kids.forEach(rec); })(RGNT);"
    "\nvar ck9 = rgnCheck();"
    "\nif (!ck9) return 'the map reported no checks';"
    # Bubbles are the only view since 0.23.0 (stack/ledger removed 2026-08-19, Derek's
    # call): the drawn-area claim is always the bubble one — circle area across the whole
    # map. RGN.view still reports the FILE's stored key, accepted and ignored, so the
    # frozen goldens' `view` pins keep checking that the reader reads the file faithfully.
    "\nvar bb9 = rgnBubbleCheck();"
    "\nvar area9 = bb9.area;"
    "\nvar akind9 = 'circle area over the whole map';"
    # the map must survive a state round trip like everything else the page saves
    "\nvar rt9 = JSON.stringify(collectState().regions) === JSON.stringify(RGN);"
    "\nreturn 'REGIONS OK ' + JSON.stringify({counts:lv9, total:rgnN(RGNT),"
    "  leaves:ck9.leaves, camps:ck9.camps, exact:!!ck9.exact, worst:ck9.worst,"
    "  area:area9, areaKind:akind9, at:RGN.at, view:RGN.view,"
    "  board_at:(TBL ? TBL.at : null),"
    "  lumis:(S.lumis || null), style:(S.style || null),"
    "  scaled:(bb9 ? bb9.scaled : null), penned:(bb9 ? bb9.shelf : null),"
    "  roundtrip:rt9});"
)

#: table mode: the page model is pure (no DOM), so it is pinned directly — page count,
#: rows per page, the innermost variable, and every first-page row's raw total, which
#: retraces the SAME integers the region pins hold. The renderer runs too, in the stub,
#: so a DOM-only regression cannot hide behind the pure model.
PAGES_PROBE = (
    "\nvar gp9 = -1;"
    "\nfor (var ip9 = 0; ip9 < TABS.length; ip9++)"
    "  if (TABS[ip9].st && TABS[ip9].st.regions) gp9 = ip9;"
    "\nif (gp9 < 0) return 'the file no longer carries a region map tab';"
    "\nswitchTab(gp9);"
    "\nif (!RGN) return 'the map tab did not come up as a region map';"
    "\ntry { drawAll(); } catch(e){ return 'the map THREW: ' + e.message; }"
    "\nvar pm9 = rgnPageModel();"
    "\nif (!pm9) return 'table mode built no page model';"
    "\nvar rw9 = pm9.pages.map(function(p){"
    "  return p.sweeps.reduce(function(a, s){ return a + s.rows.length; }, 0); });"
    "\nvar rr9 = [];"
    "\npm9.pages[0].sweeps.forEach(function(s){ s.rows.forEach(function(r){"
    "  rr9.push(r.cells.reduce(function(a, c){ return a + c[0]; }, 0)); }); });"
    "\nvar rend9 = 'ok';"
    "\ntry { rgnPagesRender(); } catch(e){ rend9 = 'THREW: ' + e.message; }"
    "\nreturn 'PAGES OK ' + JSON.stringify({on:rgnPagesOn(), count:pm9.pages.length,"
    "  rows:rw9, inner:pm9.inner, auto:pm9.auto, autopick:pm9.autoPick,"
    "  raws:rr9, render:rend9});"
)

#: the bubble grid + heat map (0.27.0): the cell every live bubble landed in is derived
#: from its DRAWN position against the axes — measuring the layout, not the model — and
#: the heat model (categories + per-category log windows) is pure, so both compare to
#: numpy-computed pins. The two color strings are regression pins on the tint formula.
BGRID_PROBE = (
    "\nvar gb9 = -1;"
    "\nfor (var ib9 = 0; ib9 < TABS.length; ib9++)"
    "  if (TABS[ib9].st && TABS[ib9].st.regions) gb9 = ib9;"
    "\nif (gb9 < 0) return 'the file no longer carries a region map tab';"
    "\nswitchTab(gb9);"
    "\nif (!RGN) return 'the map tab did not come up as a region map';"
    "\ntry { drawAll(); } catch(e){ return 'the map THREW: ' + e.message; }"
    "\nvar wi9 = rgnWinIndex();"
    "\nvar bg9 = rgnBgrid(wi9);"
    "\nif (!bg9) return 'no bubble grid on this map';"
    "\nvar ax9 = RGNAXES;"
    "\nif (!ax9) return 'the bubble grid drew no axes';"
    "\nvar cells9 = {};"
    "\nrgnBubbleOrder(RGNT).forEach(function(n){"
    "  if (!(rgnY(n) > 0) || rgnDim(n) || !n.bub || n.bub.shelf || n.bub.pin) return;"
    "  var c9 = Math.floor((n.bub.x - ax9.x0) / ax9.cw);"
    "  var r9 = Math.floor((n.bub.y - ax9.y0) / ax9.ch);"
    "  var k9 = r9 + ',' + c9;"
    "  (cells9[k9] = cells9[k9] || []).push(rgnN(n));"
    "});"
    "\nvar pm9 = rgnPageModel();"
    "\nif (!pm9) return 'no page model for the heat pins';"
    "\nvar hm9 = rgnHeatModel(pm9);"
    "\nvar win9 = {};"
    "\nObject.keys(hm9.win).forEach(function(k){"
    "  win9[k] = hm9.win[k] ? hm9.win[k].map(function(v){ return +v.toFixed(6); }) : null; });"
    "\nvar top9 = rgnHeatBg('bkg', Math.pow(10, hm9.win.bkg[1]), hm9, false);"
    "\nvar lit9 = rgnHeatBg('bkg', Math.pow(10, hm9.win.bkg[1]), hm9, true);"
    "\nreturn 'BGRID OK ' + JSON.stringify({ncol:ax9.ncol, nrow:ax9.nrow,"
    "  xname:ax9.xname, yname:ax9.yname, cols:ax9.cols, rows:ax9.rows, cells:cells9,"
    "  srcs:pm9.srcs, cats:hm9.cats, win:win9, top:top9, lit:lit9});"
)

#: the significance readout (0.29.0): rgnSigCalc is pure and consumes the SAME
#: flattened snapshot the json button exports (rgnFlat), so pinning its numbers pins
#: the whole snapshot -> algorithm path. z1 = the exact no-syst Asimov discovery Z,
#: z2 = one lnN 1.5 per background process profiled by coordinate Newton; the pins
#: were computed independently in numpy/scipy (2026-08-20) and are compared at 1e-9.
SIG_PROBE = (
    "\nvar gz9 = -1;"
    "\nfor (var iz9 = 0; iz9 < TABS.length; iz9++)"
    "  if (TABS[iz9].st && TABS[iz9].st.regions) gz9 = iz9;"
    "\nif (gz9 < 0) return 'the file no longer carries a region map tab';"
    "\nswitchTab(gz9);"
    "\nif (!RGN) return 'the map tab did not come up as a region map';"
    "\ntry { drawAll(); } catch(e){ return 'the map THREW: ' + e.message; }"
    "\nvar sm9 = rgnSigModel();"
    "\nif (!sm9) return 'no significance model';"
    "\nvar sp9 = sm9.points.map(function(p){ return {name:p.name,"
    "  z1:+p.z1.toPrecision(12), z2:(p.z2 == null ? null : +p.z2.toPrecision(12)),"
    "  dropped:p.dropped, thin:p.thin}; });"
    "\nvar sh9 = 'skipped';"
    "\ntry { sh9 = typeof rgnSigHtml() === 'string' ? 'ok' : 'not a string'; }"
    " catch(e){ sh9 = 'THREW: ' + e.message; }"
    "\nreturn 'SIG OK ' + JSON.stringify({bins:sm9.bins, bkgs:sm9.bkgs,"
    "  points:sp9, html:sh9});"
)

#: the data-less .rvr (0.28.0): no boot — the file has no columns to boot on. The page's
#: pure remoteSpecCheck reads the summary out of META, and must also still REFUSE a
#: spec whose branches are gone. The network path is deliberately unpinned: no helper
#: runs in the battery; this pins the FORMAT and the detector.
REMOTE_PROBE = (
    "\nvar out9;"
    "\ntry { out9 = remoteSpecCheck(META); }"
    " catch(e){ return 'remoteSpecCheck THREW: ' + e.message; }"
    "\nvar bad9 = JSON.parse(JSON.stringify(META));"
    "\ndelete bad9.remote.branches;"
    "\nvar refuse9 = 'no';"
    "\ntry { remoteSpecCheck(bad9); } catch(e){ refuse9 = 'yes'; }"
    "\nreturn 'REMOTE OK ' + JSON.stringify({sum: out9, refuses: refuse9});"
)


def run_remote(html: str, rvr: str) -> tuple[str, dict, int]:
    """The stub with META set from a data-less file and ONLY the remote probe — the
    normal boot cannot run on a file that deliberately carries no columns."""
    h = pathlib.Path(html).read_text()
    ids = sorted(set(re.findall(r'id="([A-Za-z_][\w-]*)"', h)))
    src = re.findall(r"<script>(.*?)</script>", h, re.S)[-1]
    blob = gzip.decompress(pathlib.Path(rvr).read_bytes())
    length = int.from_bytes(blob[:4], "little")
    meta = json.loads(blob[4:4 + length])
    body = len(blob) - 4 - length
    cut = src.rindex("(async function()")
    js = ("(function(){\nvar IDS = " + json.dumps(ids) + ";\nvar OPENIDS = [];\n"
          + STUB + src[:cut]
          + "\nMETA = " + json.dumps(meta) + ";"
          + REMOTE_PROBE + "\n})();")
    p = "/tmp/smoke_remote.js"
    pathlib.Path(p).write_text(js)
    import subprocess
    r = subprocess.run(["osascript", "-l", "JavaScript", p],
                       capture_output=True, text=True)
    return (r.stdout or r.stderr).strip(), meta, body


#: The frozen files and exactly what the version that wrote them read out. The sha means
#: the bytes may never change; the counts mean a future page must still READ them
#: identically — exact integers, the only comparison that catches a silent misread.
#: `table` is present only for files carrying a table tab, and is what the board itself
#: reports back through TABLE_PROBE, not what Python parses out of the header.
GOLDEN = {
    "golden_0.13.0_tabs.rvr": {
        "sha256": "67535ae6fbd4250b0405ea1550faaa3a2f7a8504acdf34e05bf9938587992773",
        "rowver": 4, "n": 23263, "datasets": 4, "tabs": 2, "tab": 1,
        "made_by": "0.13.0", "weights": "weight", "shown": 1738,
    },
    "golden_0.13.0_table.rvr": {
        "sha256": "30a2f3515cbbdfb9c7a652f8ae433c8220417e30e2c994c47ef1c799e9171b80",
        "rowver": 4, "n": 23263, "datasets": 4, "tabs": 2, "tab": 0,
        "made_by": "0.13.0", "weights": "weight", "shown": 1738,
        "table": {"rows": 2, "cols": 2,
                  "cells": ["signal", "ZZ", "MC \u00b7 ZZ\u21922e2mu", None],
                  "bkgs": ["ZZ", "MC \u00b7 ZZ\u21922e2mu"], "channel": "metrics",
                  "err": "poisson_neff", "plots": 1,
                  # no met key in the file: absence must still mean all three, forever
                  "met": ["sob", "sig", "zbi"],
                  "cellN": {"signal": 989, "ZZ": 453, "MC \u00b7 ZZ\u21922e2mu": 296},
                  "shelfN": 749},
    },
    "golden_0.13.0_met.rvr": {
        "sha256": "b5feee9f62b226a951ed3720a1be2127e456962f2395120410d8c791141972bc",
        "rowver": 4, "n": 23263, "datasets": 4, "tabs": 2, "tab": 0,
        "made_by": "0.13.0", "weights": "weight", "shown": 1738,
        "table": {"rows": 2, "cols": 2,
                  "cells": ["signal", "ZZ", "MC \u00b7 ZZ\u21922e2mu", None],
                  "bkgs": ["ZZ", "MC \u00b7 ZZ\u21922e2mu"], "channel": "metrics",
                  "err": "poisson_neff", "plots": 1,
                  # written ZBi FIRST on purpose: a reader that tidies this into the
                  # canonical order has silently rearranged someone's saved board
                  "met": ["zbi", "sob"],
                  "cellN": {"signal": 989, "ZZ": 453, "MC \u00b7 ZZ\u21922e2mu": 296},
                  "shelfN": 749},
    },
    #: the region map's partition: a binary tree of cuts inside a tab's state. `regions`
    #: is recomputed from the columns in Python below — never read back off the page.
    "golden_0.18.0_regions.rvr": {
        "sha256": "215e53eefbed5f343701a5570d41f72b62f215176eff56a11f3841ba792247bc",
        "rowver": 4, "n": 23263, "datasets": 4, "tabs": 2, "tab": 0,
        "made_by": "0.18.0", "weights": "weight", "shown": 2313,
        "regions": {
            # depth first, the predicate side before its complement. The ORDER is the
            # tree's, so a reader that rearranges a saved partition goes red here.
            "counts": [3, 343, 186, 320, 0, 575, 886],
            "total": 2313, "leaves": 7,
            # one empty region, on purpose: `M4L < 0` takes nothing, and a zero-area
            # child is exactly where a layout engine breaks
            "empty": 1,
            "shape": ("(channel in [0] ? (M4L<110 ? . : (M4L<140 ? (MZ2<30 ? . : .)"
                      " : .)) : (channel in [1] ? (M4L<0 ? . : .) : .))"),
            "at": 1, "view": "stack", "board_at": 0,
        },
    },
    #: 0.22.0's keys, all optional, all in one file: the grid a partition was DECLARED
    #: from, the bubble layout's switches, a floor watching named samples with `all`, a
    #: per-group luminosity, and the style channel. `lumis` is the dangerous one — a
    #: reader that ignores it still produces plausible yields, just wrong ones, so what is
    #: pinned is a yield that no single luminosity could give.
    "golden_0.22.0_grid.rvr": {
        "sha256": "8d585b09da9674d3f46cf1e06fe609c23a73173e245891dd50dd5f0c7c639713",
        "rowver": 4, "n": 23263, "datasets": 4, "tabs": 2, "tab": 0,
        "made_by": "0.22.0", "weights": "weight", "shown": 2313,
        "regions": {
            "counts": [3, 529, 213, 107, 3, 329, 151, 92, 886],
            "total": 2313, "leaves": 9, "empty": 0,
            "shape": ("(channel in [0] ? (M4L<110 ? . : (M4L<140 ? . : (M4L<170 ? . : .)))"
                      " : (channel in [1] ? (M4L<110 ? . : (M4L<140 ? . : (M4L<170 ? . :"
                      " .))) : .))"),
            "at": 0, "view": "bubble", "board_at": None,
        },
        #: what the per-group luminosity does, as yields the tab's own lumi of 1 cannot
        #: reach: signal at 3.0 and ZZ at 7.0, the loose sample left at the tab's
        "lumis": {"signal": 3.0, "ZZ": 7.0},
        #: yields.py (the Combine-format dump) on this same frozen file: eight bins that
        #: retrace the pinned grid regions, so the dump's raw counts are cross-tied to
        #: `regions.counts` above, and the whole output text is pinned by hash — a change
        #: in float formatting, lumi resolution or NaN handling goes red here.
        "yields_dump": {
            "bins": "golden_yields_bins.json",
            "raw": {"chan0": [3, 529, 213, 107], "chan1": [3, 329, 151, 92]},
            "sha256": "c5f31725991650402a90f7939767e77ca0b49634d3ab9cd21fd786668af0b6aa",
        },
        "yields": {"signal": 0.935690, "ZZ": 4.466608, "MC \u00b7 ZZ\u21922e2mu": 0.571064},
        "style": {"signal": "dash", "ZZ": "cross", "MC \u00b7 ZZ\u21922e2mu": "dots"},
        "grid": {"declared": 4, "vars": ["channel", "M4L"], "exhaustive": False,
                 "floorsrcs": ["signal", "ZZ"], "floormode": "all", "axes": True},
    },
    #: 0.24.0's table mode: the frozen grid partition with `pages` on, inner = M4L. The
    #: raws retrace the grid golden's pinned region counts through the page model's own
    #: nested-loop ordering — channel sweeps whole, M4L fastest, the no-M4L complement
    #: leaf as its own one-row sweep at the end.
    "golden_0.24.0_pages.rvr": {
        "sha256": "bf85113a64389fbe5321abc401933dfb0020158b4e219eafdc9f9ae33f2b4e15",
        "rowver": 4, "n": 23263, "datasets": 4, "tabs": 2, "tab": 0,
        "made_by": "0.24.0", "weights": "weight", "shown": 2313,
        #: `inner` is the file's EXPLICIT pick (it must always win); `autopick` is what
        #: the cleanest-tables criterion would compute for this partition — pinning it
        #: pins the criterion itself against silent drift. Here they coincide: M4L rows
        #: give 3 sweeps against channel's 5.
        "pages": {"on": True, "count": 1, "rows": [9], "inner": "M4L",
                  "auto": False, "autopick": "M4L",
                  "raws": [3, 529, 213, 107, 3, 329, 151, 92, 886]},
    },
    #: 0.27.0's bubble grid + heat map, on the SAME partition as the grid golden so the
    #: cell counts retrace already-pinned integers. bgrid axes are the REVERSE of the
    #: declared grid's (M4L across, channel down) and the file carries grid+axes too, so
    #: ncol=5 also pins that the bubble grid OUTRANKS the declared-axes layout; the
    #: 2e2mu leaf carries no M4L cut and lands in the trailing "any M4L" band. Heat:
    #: cats pin the default (in-RGN.bkg = red) plus one explicit cats override (2e2mu
    #: flipped to blue); the windows are numpy-computed per category (log10, 4-decade
    #: floor, per-group lumis folded); top/lit pin the tint formula's two color forms.
    "golden_0.27.0_bgrid.rvr": {
        "sha256": "20d28feecf934d2001b67eabe807a880b785e7a196b24a6a4999c8e469186d8e",
        "rowver": 4, "n": 23263, "datasets": 4, "tabs": 2, "tab": 0,
        "made_by": "0.27.0", "weights": "weight", "shown": 2313,
        "regions": {
            "counts": [3, 529, 213, 107, 3, 329, 151, 92, 886],
            "total": 2313, "leaves": 9, "empty": 0,
            "shape": ("(channel in [0] ? (M4L<110 ? . : (M4L<140 ? . : (M4L<170 ? . : .)))"
                      " : (channel in [1] ? (M4L<110 ? . : (M4L<140 ? . : (M4L<170 ? . :"
                      " .))) : .))"),
            "at": 0, "view": "bubble", "board_at": None,
        },
        "lumis": {"signal": 3.0, "ZZ": 7.0},
        "bgrid": {
            "ncol": 5, "nrow": 3, "xname": "M4L", "yname": "channel",
            "cols": ["M4L < 110", "110 ≤ M4L < 140", "140 ≤ M4L < 170",
                     "M4L ≥ 170", "any M4L"],
            "rows": ["channel = 4mu", "channel = 4e", "channel = 2e2mu"],
            "cells": {"0,0": [3], "0,1": [529], "0,2": [213], "0,3": [107],
                      "1,0": [3], "1,1": [329], "1,2": [151], "1,3": [92],
                      "2,4": [886]},
            "srcs": ["MC · ZZ→2e2mu", "ZZ", "signal"],
            "cats": ["sig", "bkg", "sig"],
            "win": {"sig": [-3.123339, -0.243315], "bkg": [-1.937738, 0.08962]},
            "top": "background:rgba(213,94,0,0.520)",
            "lit": "background:#e4a677",
        },
        #: the significance readout (0.29.0), pinned on the SAME frozen bytes: with the
        #: stored cats override the map has ONE background (ZZ) and two signal points.
        #: The chan-2e2mu leaf carries s>0 over b=0, so each point drops exactly one
        #: bin (never silently — the count is part of the answer); the 2e2mu point's
        #: whole s sits in that dropped bin, so its Z is exactly 0. `signal`'s numbers
        #: were computed independently in numpy (closed form) and scipy+Newton
        #: (profiled lnN 1.5 on ZZ), Newton-vs-scipy agreement 3e-12.
        "sigz": {
            "bins": 9, "bkgs": ["ZZ"],
            "points": [
                {"name": "MC · ZZ→2e2mu", "z1": 0.0, "z2": 0.0,
                 "dropped": 1, "thin": 0},
                {"name": "signal", "z1": 0.420639345625448,
                 "z2": 0.392661196950171, "dropped": 1, "thin": 1},
            ],
        },
    },
    #: 0.28.0's data-less remote file: rowver stays 4, the buffer is EMPTY, and the one
    #: optional `remote` key carries the whole study identity. Pins the format (empty
    #: body, n=0), the detector's summary (both fileset shapes: verbatim files and a
    #: named dataset), and that a branchless spec is refused. Checked by its own
    #: dataless branch below — a file with no columns cannot take the boot pipeline.
    "golden_0.28.0_dataless.rvr": {
        "sha256": "ab05626e2b06bf2482665818c6016de6f56b5484fefc09b691e1191cbe016ecc",
        "dataless": {
            "rowver": 4, "made_by": "0.28.0", "n": 0, "bytes": 0, "datasets": 2,
            "sum": {"datasets": 2, "files": 1, "columns": 3, "cut": "M4L >= 70",
                    "port": 8271, "start": "python remote/helper.py"},
            "refuses": "yes",
        },
    },
    #: the reason v4 exists — no weights column at all. Every pinned number here is an
    #: integer the page reaches without ever touching a weight.
    #: rowver 5's reason to exist: real data and weighted MC on one cart. Both samples
    #: read the same open-data file, so they hold the same events and only the weighting
    #: differs — data must read its raw count at EVERY luminosity, MC must scale.
    "golden_0.34.0_mixed.rvr": {
        "sha256": "a279233ee7a75a863c5c98f983b0f1ee7e2ecb6b32e7ae8e7c62c0211121b1cf",
        "rowver": 5, "n": 1000858, "datasets": 2, "tabs": 1, "tab": 0,
        "made_by": "0.34.0", "weights": "weight", "shown": 1000858,
        "mixed": {
            "unw": [True, False],
            "sumw": [500429, 1000858],
            "lumis": [1, 400],
            "groups": ["data", "MC"],
            "raw": 1000858,
            "at10": {"data": 500429, "MC": 10008580},
            "at400": {"data": 500429, "MC": 400343200},
            #: the badge Derek asked for: only a group holding BOTH kinds earns it
            "apart": "",
            "rawmode": "",
            "both": ('group "both" mixes weighted and unweighted samples. No weights: '
                     "Data 2012 DoubleMuParked \u2014 counted raw, never scaled by the "
                     "luminosity. Weighted: MC stand-in x2. This group's total is part "
                     "yield and part count."),
            #: and it PREFIXES the plot's own caveat rather than replacing it
            "onplot": True,
            #: a page re-save writes THIS viewer's version and carries the stamp through
            "saves": 5,
            "keepsStamp": True,
        },
    },
    "golden_0.13.0_data.rvr": {
        "sha256": "5d4889c5b32e35000686133b0c9f4544c08b4c26c0fb6a6b1310fd36e2b83f17",
        "rowver": 4, "n": 31315, "datasets": 1, "tabs": 2, "tab": 0,
        "made_by": "0.13.0", "weights": None, "shown": 21911,
    },
}


def region_tab(meta: dict) -> dict:
    """The saved state of the tab that carries a partition, or {} if none does."""
    for saved in meta.get("tabs") or []:
        if (saved.get("state") or {}).get("regions"):
            return saved["state"]
    return {}


def region_mask(meta: dict, columns: dict, st: dict):
    """The map's tab's cuts as one boolean per event — inclusive, a handle at its stop is
    open. The map reads the rail like everything else on the page does."""
    import numpy as np
    keep = np.ones(meta["n"], bool)
    for i, span in (st.get("cuts") or {}).items():
        col = meta["cols"][int(i)]
        stop_lo, stop_hi = col["nice"] or col["lim"]
        values = columns[col["name"]]
        if span[0] > stop_lo:
            keep &= values >= span[0]
        if span[1] < stop_hi:
            keep &= values <= span[1]
    for i, vals in (st.get("chips") or {}).items():
        col = meta["cols"][int(i)]
        if len(vals) < len(col["vals"] or ()):
            keep &= np.isin(columns[col["name"]], vals)
    return keep


def region_walk(node, columns: dict, here) -> list:
    """Every leaf region's raw count, depth first, the predicate side before its complement."""
    if node is None:
        return [int(here.sum())]
    cut = node["cut"]
    values = columns[cut["col"]]
    if "in" in cut:
        import numpy as np
        took = here & np.isin(values, cut["in"])
    else:
        took = here & (values < cut["lt"])
    # the other side is the COMPLEMENT, never a second comparison — that is what makes a
    # partition exhaustive whatever the column holds, NaN included
    return (region_walk(node["yes"], columns, took)
            + region_walk(node["no"], columns, here & ~took))


def region_shape(node) -> str:
    """The tree's structure as one line, so a rearranged partition cannot pass on counts."""
    if node is None:
        return "."
    cut = node["cut"]
    said = f"{cut['col']} in {cut['in']}" if "in" in cut else f"{cut['col']}<{cut['lt']:g}"
    return f"({said} ? {region_shape(node['yes'])} : {region_shape(node['no'])})"


def golden() -> str:
    """Open every frozen .rvr with today's viewer and both readers, against the pins."""
    import hashlib
    here = pathlib.Path(__file__).parent
    out = []
    for name, want in GOLDEN.items():
        f = here / "golden" / name
        if not f.exists():
            return f"{name}: MISSING — the golden file is gone"
        sha = hashlib.sha256(f.read_bytes()).hexdigest()
        if sha != want["sha256"]:
            return f"{name}: bytes CHANGED (sha {sha[:12]}…) — frozen means frozen"
        if "dataless" in want:
            # its own lane: a file with no columns cannot take the boot pipeline
            pin = want["dataless"]
            t, meta, body = run_remote(str(here / "viewer.html"), str(f))
            if not t.startswith("REMOTE OK "):
                return f"{name}: the remote detector — {t}"
            said = json.loads(t[len("REMOTE OK "):])
            got = {"rowver": meta.get("rowver"), "made_by": meta.get("made_by"),
                   "n": meta.get("n"), "bytes": meta.get("bytes"),
                   "datasets": len(meta.get("datasets") or []),
                   "sum": said["sum"], "refuses": said["refuses"]}
            if body != 0:
                return f"{name}: a data-less file carries {body} B of buffer"
            bad = {k: f"{got[k]!r} != {pin[k]!r}" for k in pin if got.get(k) != pin[k]}
            if bad:
                return f"{name}: dataless pins moved: {bad}"
            out.append(f"{name} ok — data-less, remote summary + refusal pinned")
            continue
        blob = gzip.decompress(f.read_bytes())
        meta = json.loads(blob[4:4 + int.from_bytes(blob[:4], "little")])
        got = {"rowver": meta.get("rowver"), "n": meta.get("n"),
               "datasets": len(meta.get("datasets") or []),
               "tabs": len(meta.get("tabs") or []), "tab": meta.get("tab"),
               "made_by": meta.get("made_by"),
               "weights": meta.get("weights") or None}
        bad = {k: f"{got[k]} != {want[k]}" for k in got if got[k] != want[k]}
        if bad:
            return f"{name}: META moved: {bad}"
        import page
        if len(page.read(f)[1]) != len(meta["cols"]) + (1 if want["weights"] else 0):
            return f"{name}: page.read no longer returns every column"
        r = run(str(here / "viewer.html"), str(f))
        if not r.startswith("ALL RAN"):
            return f"{name}: the page can no longer open it — {r}"
        m = re.search(r"events=(\d+) shown=(\d+)", r)
        if not m or (int(m.group(1)), int(m.group(2))) != (want["n"], want["shown"]):
            return (f"{name}: the page reads different numbers — "
                    f"{m.groups() if m else 'no counts in the summary'}")
        note = ""
        if "table" in want:
            t = run(str(here / "viewer.html"), str(f), probe=TABLE_PROBE)
            if not t.startswith("TABLE OK "):
                return f"{name}: the table tab — {t}"
            got = json.loads(t[len("TABLE OK "):])
            if got != want["table"]:
                bad = {k: f"{got.get(k)!r} != {v!r}"
                       for k, v in want["table"].items() if got.get(k) != v}
                return f"{name}: the board reads differently: {bad}"
            note = f", board {got['rows']}x{got['cols']} on {got['channel']}"
        if "regions" in want:
            pin = want["regions"]
            st = region_tab(meta)
            if not st:
                return f"{name}: no tab carries a partition any more"
            rg = st["regions"]
            columns = page.read(f)[1]
            counts = region_walk(rg["root"], columns, region_mask(meta, columns, st))
            shape = region_shape(rg["root"])
            if counts != pin["counts"]:
                return (f"{name}: the regions read {counts} against the pinned "
                        f"{pin['counts']}")
            # the property the binary rule buys, and the whole reason there is no leftover
            if sum(counts) != pin["total"]:
                return (f"{name}: the leaf regions hold {sum(counts):,} of the "
                        f"{pin['total']:,} events the tab keeps — a partition lost events")
            for key, got_v in (("leaves", len(counts)),
                               ("empty", sum(1 for c in counts if c == 0)),
                               ("shape", shape),
                               ("at", rg.get("at")), ("view", rg.get("view")),
                               ("board_at", (st.get("table") or {}).get("at"))):
                if got_v != pin[key]:
                    return f"{name}: regions {key} is {got_v!r}, pinned {pin[key]!r}"
            # and now the page's own arithmetic, against the very same integers
            t = run(str(here / "viewer.html"), str(f), probe=REGION_PROBE)
            if not t.startswith("REGIONS OK "):
                return f"{name}: the region map — {t}"
            said = json.loads(t[len("REGIONS OK "):])
            if said["counts"] != counts:
                return (f"{name}: the MAP reads {said['counts']} where numpy reads "
                        f"{counts} — the page and the file disagree")
            if not said["exact"] or said["worst"] != 0:
                return (f"{name}: the map's own sum rule is broken — worst "
                        f"|children-parent| = {said['worst']} on raw counts")
            # area is the yield, or the picture is lying about the numbers. Which area
            # depends on the view, and the probe says which one it measured.
            if said["area"] > 1e-9:
                return (f"{name}: {said['areaKind']} differs from its share of the yield "
                        f"by {said['area']:.2e} — area is not events")
            if not said["roundtrip"]:
                return f"{name}: the partition did not survive collectState"
            # the 0.22.0 keys: a reader that drops one still opens the file and still
            # produces numbers, so each is asserted rather than assumed
            if "lumis" in want and said["lumis"] != want["lumis"]:
                return (f"{name}: the per-group luminosity read {said['lumis']} against "
                        f"the pinned {want['lumis']}")
            if "style" in want and said["style"] != want["style"]:
                return (f"{name}: the style channel read {said['style']} against the "
                        f"pinned {want['style']}")
            for key in ("leaves", "at", "view", "board_at"):
                if said[key] != pin[key]:
                    return f"{name}: the map says {key}={said[key]!r}, pinned {pin[key]!r}"
            note = (f", {len(counts)} regions summing to {sum(counts):,} exactly"
                    f", area=events to {said['area']:.0e}{note}")
        if "mixed" in want:
            pin = want["mixed"]
            t = run(str(here / "viewer.html"), str(f), probe=MIXED_PROBE)
            if not t.startswith("MIXED OK "):
                return f"{name}: the mixed cart — {t}"
            said = json.loads(t[len("MIXED OK "):])
            bad = {k: f"{said.get(k)!r} != {v!r}" for k, v in pin.items() if said.get(k) != v}
            if bad:
                return f"{name}: the mixed cart reads differently: {bad}"
            note = (f", data holds {pin['at10']['data']:,} at BOTH 10 and 400 fb-1 while "
                    f"MC scales{note}")
        if "pages" in want:
            t = run(str(here / "viewer.html"), str(f), probe=PAGES_PROBE)
            if not t.startswith("PAGES OK "):
                return f"{name}: table mode — {t}"
            said = json.loads(t[len("PAGES OK "):])
            pin = want["pages"]
            for key in ("on", "count", "rows", "inner", "auto", "autopick", "raws"):
                if said[key] != pin[key]:
                    return (f"{name}: table mode {key} is {said[key]!r}, "
                            f"pinned {pin[key]!r}")
            if said["render"] != "ok":
                return f"{name}: the table renderer {said['render']}"
            note += (f", table mode: {said['count']} page(s), rows {said['rows']}, "
                     f"raws retrace the partition")
        if "bgrid" in want:
            t = run(str(here / "viewer.html"), str(f), probe=BGRID_PROBE)
            if not t.startswith("BGRID OK "):
                return f"{name}: the bubble grid — {t}"
            said = json.loads(t[len("BGRID OK "):])
            pin = want["bgrid"]
            for key in ("ncol", "nrow", "xname", "yname", "cols", "rows", "cells",
                        "srcs", "cats", "top", "lit"):
                if said[key] != pin[key]:
                    return (f"{name}: bubble grid {key} is {said[key]!r}, "
                            f"pinned {pin[key]!r}")
            # the windows are float64 log10 sums from two implementations (numpy wrote
            # the pin, the page computed its own) — compared at the pin's own precision
            for k, v in pin["win"].items():
                got_w = said["win"].get(k)
                ok = (got_w is None and v is None) or (
                    got_w is not None and v is not None and len(got_w) == len(v)
                    and all(abs(a - b) <= 5e-6 for a, b in zip(got_w, v)))
                if not ok:
                    return f"{name}: heat window {k} is {got_w!r}, pinned {v!r}"
            note += ", bubble grid cells + heat model pinned"
        if "sigz" in want:
            t = run(str(here / "viewer.html"), str(f), probe=SIG_PROBE)
            if not t.startswith("SIG OK "):
                return f"{name}: the significance readout — {t}"
            said = json.loads(t[len("SIG OK "):])
            pin = want["sigz"]
            if said["bins"] != pin["bins"] or said["bkgs"] != pin["bkgs"]:
                return (f"{name}: significance sees bins={said['bins']} "
                        f"bkgs={said['bkgs']}, pinned {pin['bins']}/{pin['bkgs']}")
            if said["html"] != "ok":
                return f"{name}: the readout html {said['html']}"
            if len(said["points"]) != len(pin["points"]):
                return (f"{name}: {len(said['points'])} signal points against "
                        f"{len(pin['points'])} pinned")
            for got_p, want_p in zip(said["points"], pin["points"]):
                for k in ("name", "dropped", "thin"):
                    if got_p[k] != want_p[k]:
                        return (f"{name}: significance {k} is {got_p[k]!r}, "
                                f"pinned {want_p[k]!r} for {want_p['name']!r}")
                for k in ("z1", "z2"):
                    a, b = got_p[k], want_p[k]
                    if (a is None) != (b is None) or (
                            a is not None and abs(a - b) > 1e-9):
                        return (f"{name}: significance {k} is {a!r}, pinned {b!r} "
                                f"for {want_p['name']!r} — beyond 1e-9")
            note += ", significance z1/z2 + edge counts pinned"
        if "yields_dump" in want:
            import yields
            spec = want["yields_dump"]
            bins = json.loads((here / "golden" / spec["bins"]).read_text())
            dumped = yields.dump(f, bins, tab_cuts=True, allow_tab_lumi=True)
            raw = {b: sum(int(v["nominal"][0]) for v in dumped[b].values())
                   for b in dumped}
            got = {ch: [raw[f"{ch}_{t}"] for t in ("Mlt110", "M110_140",
                                                   "M140_170", "M170")]
                   for ch in ("chan0", "chan1")}
            if got != spec["raw"]:
                return (f"{name}: the yields dump counts {got} against the pinned "
                        f"{spec['raw']} — the dump and the region map disagree")
            text = json.dumps(dumped, indent=4, sort_keys=True) + "\n"
            sha = hashlib.sha256(text.encode()).hexdigest()
            if sha != spec["sha256"]:
                return (f"{name}: the yields dump text moved (sha {sha[:12]}…) — "
                        f"formatting, lumi resolution or NaN handling changed")
            note += f", yields dump retraces the partition and its text is byte-pinned"
        out.append(f"{name} ok — {want['n']:,} events, shown={want['shown']}{note}")
    return "; ".join(out)


USAGE = """boot a page, or drop a .rvr on one, or round-trip a pikl through the format:

    smoke.py pages/template.html=template
    smoke.py pages/viewer.html+pages/template.rvr=drop
    smoke.py rowver:template=rt
    smoke.py golden=golden                       the frozen files still open, unchanged
"""


def viewtrip(rvr: str) -> str:
    """Extract a .rvr into a view.py and rebuild its tabs: anything it drops shows here."""
    import importlib.util
    import tempfile
    sys.path.insert(0, str(pathlib.Path(__file__).parent))
    import page
    meta, _ = page.read(rvr)
    src = pathlib.Path(tempfile.mkdtemp()) / "view.py"
    src.write_text(page.extract_py(rvr))
    spec = importlib.util.spec_from_file_location("_viewtrip", src)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    rebuilt = page.tabs(meta, mod.VIEW, getattr(mod, "GROUPS", None),
                        getattr(mod, "SUBGROUPS", None))
    if len(rebuilt) != len(meta.get("tabs") or []):
        return f"{len(meta.get('tabs') or [])} tabs in, {len(rebuilt)} out"
    bad = []
    for a, b in zip(meta["tabs"], rebuilt):
        if a["name"] != b["name"]:
            bad.append(f"name {a['name']!r} -> {b['name']!r}")
        for k in set(a["state"]) | set(b["state"]):
            x, y = a["state"].get(k), b["state"].get(k)
            # an empty `show` means every sample on, which is what all-True says too
            if k == "show" and not x != (y or {}) or x == y:
                continue
            if k == "show" and all(v for v in (x or {}).values()) and not y:
                continue
            bad.append(f"{a['name']}.{k}")
    return ("; ".join(sorted(set(bad))) if bad
            else f"{len(rebuilt)} tabs survived the round trip unchanged")


def check(path: str) -> str:
    """Dispatch one argument to the boot, drop, round-trip or golden check."""
    if path == "golden":
        return golden()
    if path.startswith("view:"):
        return viewtrip(path[len("view:"):])
    if path.startswith("rowver:"):
        return roundtrip(path[len("rowver:"):])
    if "+" in path:
        return run(*path.split("+", 1))
    return run(path)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(USAGE)
    for a in sys.argv[1:]:
        path, label = a.rsplit("=", 1)
        print(f"  {label:9s} {check(path)}")
