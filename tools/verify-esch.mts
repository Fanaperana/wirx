// Cross-checks a wirx-generated .esch with eleceda's own code: it must load without repairs, pass
// eleceda's validator with no errors or warnings, score well for readability, and have exactly
// the connectivity of wirx's netlist.
//
// Run from an eleceda checkout (so its packages resolve):
//   cd <eleceda> && pnpm exec tsx <wirx>/tools/verify-esch.mts out.esch [netlist.json]
//
// The netlist comes from `wirx netlist design.wx --format json -o netlist.json`.
import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const eleceda = process.env.ELECEDA_DIR ?? process.cwd();
const core = await import(pathToFileURL(join(eleceda, 'packages/core/src/index.ts')).href);
const formats = await import(pathToFileURL(join(eleceda, 'packages/formats/src/index.ts')).href);

const [file, netlistFile] = process.argv.slice(2);
if (!file) {
	console.error('usage: verify-esch.mts <file.esch> [netlist.json]');
	process.exit(2);
}

const { project, notes } = formats.loadProject(readFileSync(resolve(file), 'utf8'));
const nl = core.buildNetlist(project);
let failed = false;
const fail = (msg: string) => {
	failed = true;
	console.log(`FAIL ${msg}`);
};

if (notes.length) fail(`load repaired the file: ${notes.join('; ')}`);
else console.log('ok   loads without repairs');

const violations = core.validate(project, { netlist: nl }).violations;
const serious = violations.filter((v: { severity: string }) => v.severity !== 'info');
if (serious.length) for (const v of serious) fail(`${v.severity} ${v.rule}: ${v.message}`);
else console.log('ok   no errors or warnings');
const info = violations.filter((v: { severity: string }) => v.severity === 'info');
for (const v of info) console.log(`info ${v.rule}: ${v.message}`);

const report = core.readabilityReport(project, nl);
for (const s of report.sheets) {
	const line = `readability ${s.name}: ${s.score} (parts ${s.parts}, wires ${s.wires}, labels ${s.labels})`;
	if (s.score < 95) fail(line);
	else console.log(`ok   ${line}`);
}

if (netlistFile) {
	const want = JSON.parse(readFileSync(resolve(netlistFile), 'utf8'));
	const key = (pins: string[]) => pins.sort().join(' ');
	const expected = new Set<string>(
		want.nets.map((n: { pins: { ref: string; pin: string }[] }) => key(n.pins.map((p) => `${p.ref}.${p.pin}`)))
	);
	const got = new Set<string>(
		core
			.exportableNets(nl)
			.map((n: { nodes: { ref: string; pin: string }[] }) => key(n.nodes.map((x) => `${x.ref}.${x.pin}`)))
	);
	const missing = [...expected].filter((k) => !got.has(k));
	const extra = [...got].filter((k) => !expected.has(k));
	if (missing.length || extra.length) {
		for (const m of missing) fail(`net missing in drawing: ${m}`);
		for (const x of extra) fail(`net in drawing but not in design: ${x}`);
	} else console.log(`ok   connectivity matches wirx (${expected.size} nets)`);
}

process.exit(failed ? 1 : 0);
