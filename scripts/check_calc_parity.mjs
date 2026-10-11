// Test orchestration only. Calc produces caches; the existing Rust importer
// independently recalculates and compares typed values, including error cells.
import { readFileSync, writeFileSync, mkdirSync, existsSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const [outputArg, scorerArg, suite = 'information'] = process.argv.slice(2);
if (!outputArg || !scorerArg || ![4, 5].includes(process.argv.length) || !['information', 'common'].includes(suite)) {
  throw new Error('usage: node scripts/check_calc_parity.mjs NEW_EVIDENCE_DIR RUST_DIAGNOSTIC [information|common]');
}
const output = resolve(outputArg);
const scorer = resolve(scorerArg);
const reference = process.env.CALC_REFERENCE ?? 'libreoffice';
const referenceSettings = { useWildcards: true, useRegularExpressions: false, caseSensitive: false };
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
const fixturePath = resolve(`tests/calc-parity/${suite}.json`);
const fixtureBytes = readFileSync(fixturePath);
const cases = JSON.parse(fixtureBytes).map((item, index) => typeof item === 'string' ? { id: `information-${index + 1}`, group: 'information', formula: item } : item);
const formulas = cases.map(item => item.formula);
if (!Array.isArray(formulas) || !formulas.length || formulas.length > 5000
    || formulas.some(f => typeof f !== 'string' || !f.startsWith('=') || f.length > 4096)) {
  throw new Error('invalid bounded formula fixture');
}
const source = join(output, `${suite}.fods`);
const catalogue = JSON.parse(readFileSync('tests/calc-parity/calc-catalogue.json', 'utf8'));
const normalize = name => name.toUpperCase().replace(/^_XLFN\./, '').replace(/^_XLWS\./, '');
const opcode = new Map([...catalogue.dialects.CALC, ...catalogue.dialects.OOXML].map(item => [normalize(item.name), item.opcode]));
const odfNames = new Map(catalogue.dialects.ODFF.map(item => [item.opcode, item.name]));
function openFormula(formula) {
  let result = '', quoted = false, array = 0;
  for (let i = 0; i < formula.length; i++) {
    const c = formula[i];
    if (c === '"') {
      result += c;
      if (quoted && formula[i + 1] === '"') result += formula[++i];
      else quoted = !quoted;
    } else if (quoted) result += c;
    else if (c === '{') { array++; result += c; }
    else if (c === '}') { array--; result += c; }
    else if (c === ',') result += ';';
    else if (c === ';' && array) result += '|';
    else if (/[0-9.]/.test(c) && formula.slice(i).match(/^(?:\d+(?:\.\d*)?|\.\d+)(?:[Ee][+-]?\d+)?/)) {
      // Consume a complete numeric token before looking for A1 references:
      // the exponent in 1E16 is not a reference to cell E16.
      const token = formula.slice(i).match(/^(?:\d+(?:\.\d*)?|\.\d+)(?:[Ee][+-]?\d+)?/)[0];
      result += token;
      i += token.length - 1;
    } else if (/[A-Za-z_]/.test(c)) {
      const match = formula.slice(i).match(/^[A-Za-z_][A-Za-z0-9_.]*/)[0];
      const next = formula[i + match.length];
      if (next === '(') result += odfNames.get(opcode.get(normalize(match))) ?? match;
      else if (match === 'Inputs' && next === '!') {
        const address = formula.slice(i + match.length + 1).match(/^([A-Z]+[0-9]+)(?::([A-Z]+[0-9]+))?/);
        if (!address) throw new Error('invalid trusted Inputs reference');
        result += `[Inputs.${address[1]}${address[2] ? `:Inputs.${address[2]}` : ''}]`;
        i += 1 + address[0].length;
      } else if (/^[A-Z]+[0-9]+$/.test(match)) {
        const end = next === ':' ? formula.slice(i + match.length + 1).match(/^[A-Z]+[0-9]+/) : null;
        result += `[.${match}${end ? `:.${end[0]}` : ''}]`;
        if (end) i += 1 + end[0].length;
      } else result += match;
      i += match.length - 1;
    } else result += c;
  }
  return result;
}
if (openFormula('=SUM(1E16,1,-1e+16,.5E-2,A16)') !== '=SUM(1E16;1;-1e+16;.5E-2;[.A16])') {
  throw new Error('numeric/reference token translation regression');
}
const inputs = '<table:table table:name="Inputs"><table:table-row><table:table-cell office:value-type="float" office:value="2"/></table:table-row><table:table-row><table:table-cell office:value-type="float" office:value="3"/></table:table-row><table:table-row><table:table-cell office:value-type="string"><text:p>abc</text:p></table:table-cell></table:table-row><table:table-row><table:table-cell office:value-type="boolean" office:boolean-value="true"/></table:table-row><table:table-row><table:table-cell/></table:table-row></table:table>';
const rows = formulas.map(formula => `<table:table-row><table:table-cell table:formula="of:${xml(openFormula(formula))}" office:value-type="float" office:value="0"><text:p>0</text:p></table:table-cell></table:table-row>`).join('\n');
writeFileSync(source, `<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2" office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.spreadsheet"><office:body><office:spreadsheet><table:calculation-settings table:use-wildcards="true" table:use-regular-expressions="false" table:case-sensitive="false"/><table:table table:name="Information">${rows}</table:table>${suite === 'common' ? inputs : ''}</office:spreadsheet></office:body></office:document>\n`, { flag: 'wx' });
const referenceVersion = run(reference, ['--version']).trim();
const conversion = run(reference, [
  `-env:UserInstallation=${pathToFileURL(join(output, 'profile')).href}`,
  '--headless', '--convert-to', 'xlsx:Calc MS Excel 2007 XML', '--outdir', workbooks, source,
]);
writeFileSync(join(output, 'reference.log'), conversion, { flag: 'wx' });
const workbook = join(workbooks, `${suite}.xlsx`);
if (!existsSync(workbook)) throw new Error('Calc did not produce the reference workbook');
const diagnostic = JSON.parse(run(scorer, [workbook]));
const score = diagnostic.report;
const fullParityPassed = ['formula_cells_observed', 'formula_cells_loaded', 'formula_cells_compared', 'stored_values_matched']
  .every(field => score[field] === formulas.length)
  && score.unsupported_formulas === 0 && score.stored_values_mismatched === 0;
const groups = {};
for (const c of cases) groups[c.group] = { cases: (groups[c.group]?.cases ?? 0) + 1, mismatches: 0, unsupported: 0 };
for (const mismatch of diagnostic.mismatches) {
  mismatch.case = cases[mismatch.row - 1];
  groups[mismatch.case.group].mismatches++;
}
for (const refusal of diagnostic.unsupported) {
  refusal.case = cases[refusal.row - 1];
  groups[refusal.case.group].unsupported++;
}
const targetPassed = fullParityPassed && (suite !== 'common' || formulas.length >= 400);
const evidence = {
  schema: 1, referenceVersion, referenceSettings,
  comparison: 'Typed text, boolean and error values must match; finite numbers use |owned-reference| <= 1e-9 * max(|reference|, 1), the existing XLSX scorer tolerance',
  ownedCommit: run('git', ['rev-parse', 'HEAD']).trim(),
  fixtureSha256: createHash('sha256').update(fixtureBytes).digest('hex'),
  sourceSha256: createHash('sha256').update(readFileSync(source)).digest('hex'),
  suite, cases, groups, passed: targetPassed, fullParityPassed, score, mismatches: diagnostic.mismatches, unsupported: diagnostic.unsupported,
  scope: suite === 'common' ? 'declared common-calculation suite: minimum 400 cases, every typed result must match, no refusals; not full Calc parity' : 'strict information function reference slice; not full Calc parity',
};
writeFileSync(join(output, 'parity.json'), JSON.stringify(evidence, null, 2) + '\n', { flag: 'wx' });
console.log(JSON.stringify({ suite, passed: targetPassed, fullParityPassed, groups, score, mismatches: diagnostic.mismatches, unsupported: diagnostic.unsupported }));
if (!targetPassed) process.exitCode = 1;
