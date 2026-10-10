// REA is an investigation dependency, never a runtime dependency of the kit.
// Keep complete, unmodified Evidence beside this derived contract summary.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const directory = resolve(process.argv[2] ?? "");
assert(process.argv.length === 3, "usage: node scripts/rea_kit_contract.mjs EVIDENCE_DIR");
const rea = process.env.REA;
assert(rea, "set REA to the installed rea CLI");
const program = process.env.OMASHEETS_LOK_PROGRAM ?? "/usr/lib/libreoffice/program";
const source = join(directory, "reference.xlsx");
const output = join(directory, "reference.ppm");
const sha256 = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");
const before = sha256(source);
function investigate(command, arguments_, name) {
  // No shell; complete bytes are retained before projecting a result.
  let bytes;
  try {
    bytes = execFileSync(rea, [command, ...arguments_, "--format", "json"], {
      maxBuffer: 64 * 1024 * 1024, timeout: 90_000,
    });
  } catch (error) {
    if (error.stdout) writeFileSync(join(directory, name), error.stdout, { flag: "wx" });
    throw new Error(`REA ${command} failed: ${error.stdout?.toString() ?? error.message}`);
  }
  writeFileSync(join(directory, name), bytes, { flag: "wx" });
  const evidence = JSON.parse(bytes);
  assert(evidence.evidence_id && evidence.normalized_result, "REA did not produce Evidence");
  return evidence;
}
// Distribution builds merge the office implementation into libmergedlo.so,
// which exceeds the bounded ELF adapter's 32 MiB input budget. In that case
// inspect the GTK LOK bridge actually used by OmaSheets' compatibility window.
const split = join(program, "libsofficeapp.so");
const candidates = [split, join(program, "liblibreofficekitgtk.so"),
  "/usr/lib/liblibreofficekitgtk.so", "/usr/lib/x86_64-linux-gnu/liblibreofficekitgtk.so"];
const selected = candidates.find(existsSync);
assert(selected, "no installed LOK implementation or GTK bridge was found");
const reference = realpathSync(selected);
const artifact = investigate("inspect-artifact", [reference], "artifact.json");
const layout = investigate("inspect-binary-layout", [reference], "layout.json");
assert.equal(layout.operation, "inspect_binary_layout");
assert.equal(layout.subject.digest.sha256, artifact.subject.digest.sha256);
const seeds = existsSync(split) ? ["libreofficekit_hook", "libreofficekit_hook_2"]
  : ["lok_doc_view_open_document", "lok_doc_view_get_document", "lok_doc_view_post_command"];
const hooks = layout.normalized_result.symbols.filter((symbol) =>
  seeds.some((seed) => JSON.stringify(symbol.name).includes(seed)));
assert(hooks.length >= seeds.length, "reference must expose the used LOK entry points");

const scenario = {
  executable: join(directory, "omasheets-lok-render"),
  arguments: [source, output, "800", "500"],
  working_directory: directory,
  environment: { OMASHEETS_LOK_PROGRAM: program, SAL_USE_VCLPLUGIN: "svp" },
  filesystem_observation_paths: [source, output],
  timeout_ms: 30_000, idle_timeout_ms: 30_000,
};
const scenarioPath = join(directory, "scenario.json");
writeFileSync(scenarioPath, JSON.stringify(scenario, null, 2) + "\n", { flag: "wx" });
const capture = investigate("capture-process", [scenarioPath], "capture.json");
assert.equal(capture.operation, "capture_process_scenario");
assert.equal(capture.normalized_result.exit.code, 0);
assert.equal(capture.normalized_result.truncated, false);
const text = capture.normalized_result.frames.map((frame) => frame.data).join("");
const reports = text.split(/\r?\n/u).filter((line) => line.trim().startsWith("{"))
  .map((line) => JSON.parse(line.trim())).filter((value) => value.engine === "libreofficekit");
assert.equal(reports.length, 1, "capture must contain exactly one renderer report");
const report = reports[0];
assert.equal(report.engine, "libreofficekit");
assert.equal(report.parts, 1);
assert.equal(report.width, 800);
assert.equal(report.height, 500);
const ppm = readFileSync(output);
const header = Buffer.from("P6\n800 500\n255\n");
assert(ppm.subarray(0, header.length).equals(header));
assert.equal(ppm.length, header.length + 800 * 500 * 3);
assert.equal(sha256(source), before, "rendering must preserve source bytes");

const summary = {
  schema: 1, rea_version: "6.3.0", reference_sha256: layout.subject.digest.sha256,
  evidence: { artifact: artifact.evidence_id, layout: layout.evidence_id, capture: capture.evidence_id },
  entry_points: hooks, render: report, source_preserved: true,
  limitations: [
    "This is one bounded spreadsheet rendering observation, not full LibreOfficeKit parity.",
    "ELF linkage does not establish function implementation or runtime addresses.",
    "Input events, saveAs dirty-state effects, ODS, XLS, macros and physical Wayland were not observed.",
  ],
};
writeFileSync(join(directory, "summary.json"), JSON.stringify(summary, null, 2) + "\n", { flag: "wx" });
console.log(JSON.stringify(summary));
