// Test orchestration only. Calc produces caches; the existing Rust importer
// independently recalculates and compares typed values, including error cells.
import { readFileSync, writeFileSync, mkdirSync, existsSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const [outputArg, scorerArg] = process.argv.slice(2);
if (!outputArg || !scorerArg || process.argv.length !== 4) {
  throw new Error('usage: node scripts/check_calc_parity.mjs NEW_EVIDENCE_DIR XLSX_SCORER');
}
const output = resolve(outputArg);
const scorer = resolve(scorerArg);
mkdirSync(output, { recursive: false, mode: 0o700 });
const workbooks = join(output, 'workbooks');
mkdirSync(workbooks);
const run = (command, args) => {
  const result = spawnSync(command, args, {
    encoding: 'utf8', timeout: 120_000, maxBuffer: 2 * 1024 * 1024,
    env: { ...process.env, LANG: 'C.UTF-8', LC_ALL: 'C.UTF-8' },
  });
  if (result.error || result.status !== 0) {
    throw new Error(`${command} failed: ${result.error ?? result.stderr}`);
  }
  return result.stdout;
};
const xml = value => value.replaceAll('&', '&amp;').replaceAll('"', '&quot;')
  .replaceAll('<', '&lt;').replaceAll('>', '&gt;');
const fixturePath = resolve('tests/calc-parity/information.json');
const fixtureBytes = readFileSync(fixturePath);
const formulas = JSON.parse(fixtureBytes);
if (!Array.isArray(formulas) || !formulas.length || formulas.length > 1000
    || formulas.some(f => typeof f !== 'string' || !f.startsWith('=') || f.length > 4096)) {
  throw new Error('invalid bounded formula fixture');
}
const source = join(output, 'information.fods');
const rows = formulas.map(formula => `<table:table-row><table:table-cell table:formula="of:${xml(formula.replaceAll(',', ';'))}" office:value-type="float" office:value="0"><text:p>0</text:p></table:table-cell></table:table-row>`).join('\n');
writeFileSync(source, `<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2" office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.spreadsheet"><office:body><office:spreadsheet><table:table table:name="Information">${rows}</table:table></office:spreadsheet></office:body></office:document>\n`, { flag: 'wx' });
const referenceVersion = run('libreoffice', ['--version']).trim();
const conversion = run('libreoffice', [
  `-env:UserInstallation=${pathToFileURL(join(output, 'profile')).href}`,
  '--headless', '--convert-to', 'xlsx:Calc MS Excel 2007 XML', '--outdir', workbooks, source,
]);
writeFileSync(join(output, 'reference.log'), conversion, { flag: 'wx' });
const workbook = join(workbooks, 'information.xlsx');
if (!existsSync(workbook)) throw new Error('Calc did not produce the reference workbook');
const diagnostic = JSON.parse(run(scorer, [workbook]));
const score = diagnostic.report;
const passed = ['formula_cells_observed', 'formula_cells_loaded', 'formula_cells_compared', 'stored_values_matched']
  .every(field => score[field] === formulas.length)
  && score.unsupported_formulas === 0 && score.stored_values_mismatched === 0;
const evidence = {
  schema: 1, referenceVersion, fixtureSha256: createHash('sha256').update(fixtureBytes).digest('hex'),
  sourceSha256: createHash('sha256').update(readFileSync(source)).digest('hex'),
  formulas, passed, score, mismatches: diagnostic.mismatches, unsupported: diagnostic.unsupported,
  scope: 'scalar information functions and basic numerical regression; not full Calc parity',
};
writeFileSync(join(output, 'parity.json'), JSON.stringify(evidence, null, 2) + '\n', { flag: 'wx' });
console.log(JSON.stringify(evidence));
if (!passed) process.exitCode = 1;
