// Extract a versioned name/opcode catalogue, never implementation code.
// Usage: node scripts/inventory_calc_parity.mjs PINNED_CORE_RESOURCE_HRC OUTPUT_JSON
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
const [sourcePath, outputPath] = process.argv.slice(2);
if (!sourcePath || !outputPath || process.argv.length !== 4) throw new Error('expected source and new output paths');
const bytes = readFileSync(sourcePath);
const source = bytes.toString('utf8');
const dialects = {};
for (const match of source.matchAll(/RID_STRLIST_FUNCTION_NAMES_ENGLISH(_ODFF|_OOXML|_PODF|_API)?\[\]\s*=\s*\{([\s\S]*?)\n\};/g)) {
  const dialect = (match[1] ?? '_CALC').slice(1);
  const entries = [...match[2].matchAll(/\{\s*"([^"]*)"\s*,\s*(oc\w+)\s*\}/g)]
    .map(item => ({ name: item[1], opcode: item[2] }));
  if (!entries.length || dialects[dialect]) throw new Error(`invalid/duplicate dialect ${dialect}`);
  dialects[dialect] = entries;
}
if (Object.keys(dialects).length !== 5) throw new Error('expected all five upstream English token maps');
const owned = readFileSync('crates/omasheets-calc/src/lib.rs', 'utf8');
const registry = owned.match(/const FUNCTION_REGISTRY:[\s\S]*?= &\[([\s\S]*?)\n\];/);
if (!registry) throw new Error('owned function registry not found');
const names = [...registry[1].matchAll(/\("([^"]+)", Function::\w+\)/g)].map(item => item[1]);
const normalized = name => name.toUpperCase().replace(/^_XLFN\./, '').replace(/^_XLWS\./, '');
const ownedNames = new Set(names);
const admission = {};
for (const [dialect, entries] of Object.entries(dialects)) {
  const namedTokens = entries.filter(entry => /^[A-Za-z_][A-Za-z0-9_.]*$/.test(entry.name));
  admission[dialect] = {
    namedTokenCount: namedTokens.length,
    ownedParserNames: namedTokens.filter(entry => ownedNames.has(normalized(entry.name))).map(entry => entry.name),
    absentParserNames: namedTokens.filter(entry => !ownedNames.has(normalized(entry.name))).map(entry => entry.name),
  };
}
const result = {
  schema: 1,
  upstream: { repository: 'LibreOffice/core', commit: '1746b16a564f59fcaf8c5670bb292748408da54e',
    path: 'formula/inc/core_resource.hrc', sha256: createHash('sha256').update(bytes).digest('hex') },
  scope: 'token-name inventory only; matching names do not establish dialect or semantic parity',
  ownedRegistryNames: names, admission, dialects,
};
writeFileSync(outputPath, JSON.stringify(result) + '\n', { flag: 'wx' });
console.log(JSON.stringify({ ownedParserNames: names.length,
  dialects: Object.fromEntries(Object.entries(admission).map(([dialect, value]) => [dialect, {
    namedTokens: value.namedTokenCount, recognizedNames: value.ownedParserNames.length, absentNames: value.absentParserNames.length,
  }])) }));
