// Dump the FINAL compiled marked rule sources (raw JS regex sources, escapes
// intact) for the pi-tui markdown config (gfm=true, breaks=false, pedantic=false).
// Usage: node scripts/dump-marked-rules.mjs > /tmp/marked-rules.json
import { createRequire } from 'node:module';
const require = createRequire('/tmp/pi-repo/package.json');
const marked = require('marked');

const opts = { gfm: true, breaks: false, pedantic: false };
const lexer = new marked.Lexer(opts);
const rules = lexer.rules;

const out = { flags: {} };
for (const group of ['block', 'inline']) {
  out[group] = {};
  for (const [name, re] of Object.entries(rules[group])) {
    out[group][name] = re.source;
    if (re.flags) out.flags[`${group}.${name}`] = re.flags;
  }
}
// misc/other rules: marked exposes them on the Lexer rules object? Check keys.
out._ruleGroups = Object.keys(rules);
// The "misc" table in marked v18: try lexer.rules directly / misc export.
if (rules.misc) {
  out.misc = {};
  for (const [name, re] of Object.entries(rules.misc)) {
    out.misc[name] = re.source;
    if (re.flags) out.flags[`misc.${name}`] = re.flags;
  }
}
console.log(JSON.stringify(out, null, 1));
