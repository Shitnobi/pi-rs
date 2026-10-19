// Regenerate the static rule tables in crates/pi-tui/src/marked/rules.rs from
// the FINAL compiled marked v18 regexes (raw JS sources, escapes intact).
// The runtime `rx()` builder translates JS semantics (Unicode \s, JS dot,
// ASCII \d/\w/\b) to fancy-regex, so the dumped sources are stored verbatim.
// Usage: node scripts/gen-marked-rules.mjs
import { createRequire } from 'node:module';
import { readFileSync, writeFileSync } from 'node:fs';

const require = createRequire('/tmp/pi-repo/package.json');
const marked = require('marked');

// pi-tui markdownParser config (gfm=true, breaks=false, pedantic=false).
const rules = new marked.Lexer().tokenizer.rules;

// `other` entries that are builder functions, ported as methods instead.
const SKIP_FUNCS = new Set([
  'listItemRegex', 'nextBulletRegex', 'hrRegex', 'fencesBeginRegex',
  'headingBeginRegex', 'htmlBeginRegex', 'blockquoteBeginRegex',
]);

function rustLiteral(src) {
  let out = '';
  for (const ch of src) {
    const code = ch.codePointAt(0);
    if (ch === '\\') out += '\\\\';
    else if (ch === '"') out += '\\"';
    else if (code === 0x09) out += '\\t';
    else if (code < 0x20 || code > 0x7e) throw new Error(`unexpected char U+${code.toString(16)} in source`);
    else out += ch;
  }
  return out;
}

function genGroup(group, structName) {
  const lines = [`            ${group === 'other' ? 'other' : group}: ${structName} {`];
  for (const [name, re] of Object.entries(rules[group])) {
    if (typeof re === 'function') {
      if (!SKIP_FUNCS.has(name)) throw new Error(`unknown function rule: ${group}.${name}`);
      continue;
    }
    const ci = re.flags.includes('i');
    const ml = re.flags.includes('m');
    lines.push(`            ${name}: rx("${rustLiteral(re.source)}", ${ci}, ${ml}),`);
  }
  lines.push('            },');
  return lines.join('\n');
}

const body = [
  '    pub fn new() -> Rules {',
  '        Rules {',
  genGroup('block', 'BlockRules'),
  genGroup('inline', 'InlineRules'),
  genGroup('other', 'OtherRules'),
  '        }',
  '    }',
  '',
].join('\n');

const path = new URL('../crates/pi-tui/src/marked/rules.rs', import.meta.url).pathname;
let src = readFileSync(path, 'utf8');
const START = '    pub fn new() -> Rules {';
const END = "    /// marked's `other.listItemRegex(marker)`";
const startIdx = src.indexOf(START);
const endIdx = src.indexOf(END);
if (startIdx < 0 || endIdx < 0 || endIdx < startIdx) throw new Error('markers not found');
src = src.slice(0, startIdx) + body + '\n' + src.slice(endIdx);
writeFileSync(path, src);
console.log('rules.rs statics regenerated.');
