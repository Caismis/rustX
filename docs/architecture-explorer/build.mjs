import { readFileSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const directory = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(directory, '../..');
const baseline = '8cb41624eb2dc2ca87e4409961f42553ceb029cf';
const github = 'https://github.com/Caismis/rustX';
const args = process.argv.slice(2);
if (args.some((arg) => arg !== '--check') || args.length > 1) {
  throw new Error('Usage: node docs/architecture-explorer/build.mjs [--check]');
}
const read = (name) => readFileSync(path.join(directory, name), 'utf8');
const data = JSON.parse(read('content.json'));
const fail = (message) => { throw new Error(`Invalid explorer content: ${message}`); };
const object = (value, name) => {
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail(`${name} must be an object`);
};
const string = (value, name) => {
  if (typeof value !== 'string' || !value.trim()) fail(`${name} must be a nonempty string`);
};
const array = (value, name, allowEmpty = false) => {
  if (!Array.isArray(value) || (!allowEmpty && !value.length)) fail(`${name} must be ${allowEmpty ? 'an' : 'a nonempty'} array`);
};
const strings = (value, name) => {
  array(value, name);
  value.forEach((item, index) => string(item, `${name}[${index}]`));
};
const ids = (items, name) => {
  array(items, name);
  const result = new Set();
  items.forEach((item) => {
    object(item, name);
    string(item.id, `${name}.id`);
    if (!/^[a-z][a-z0-9-]*$/.test(item.id)) fail(`${name}: invalid id ${item.id}`);
    if (result.has(item.id)) fail(`${name}: duplicate id ${item.id}`);
    result.add(item.id);
  });
  return result;
};
const refs = (items, known, name, allowEmpty = false) => {
  array(items, name, allowEmpty);
  if (new Set(items).size !== items.length) fail(`${name}: duplicate reference`);
  for (const id of items) if (!known.has(id)) fail(`${name}: unknown reference ${id}`);
};
object(data, 'content');
string(data.title, 'title');
string(data.intro, 'intro');
object(data.baseline, 'baseline');
if (data.baseline.commit !== baseline) fail(`baseline.commit must remain ${baseline}`);
string(data.baseline.branch, 'baseline.branch');
string(data.baseline.subject, 'baseline.subject');
const nodeIds = ids(data.nodes, 'nodes');
const evidenceIds = ids(data.evidence, 'evidence');
data.nodes.forEach((node) => {
  ['label', 'layer', 'summary'].forEach((key) => string(node[key], `node ${node.id}.${key}`));
  strings(node.owns, `node ${node.id}.owns`);
  strings(node.boundaries, `node ${node.id}.boundaries`);
  refs(node.evidence, evidenceIds, `node ${node.id}.evidence`);
});
array(data.relations, 'relations');
const relationKeys = new Set();
data.relations.forEach((relation) => {
  object(relation, 'relation');
  refs([relation.from, relation.to], nodeIds, 'relation endpoints');
  if (!['call', 'data', 'dependency'].includes(relation.kind)) fail(`unknown relation kind ${relation.kind}`);
  string(relation.label, 'relation.label');
  const key = JSON.stringify(relation);
  if (relationKeys.has(key)) fail(`duplicate relation ${relation.from} → ${relation.to}`);
  relationKeys.add(key);
});
const steps = (items, name) => {
  ids(items, name);
  items.forEach((step) => {
    ['title', 'actor', 'description', 'durable', 'visible'].forEach((key) => string(step[key], `${name}.${step.id}.${key}`));
    refs(step.active, nodeIds, `${name}.${step.id}.active`);
    refs(step.evidence, evidenceIds, `${name}.${step.id}.evidence`);
  });
};
steps(data.request, 'request');
ids(data.scenarios, 'scenarios');
data.scenarios.forEach((scenario) => {
  string(scenario.title, `scenario ${scenario.id}.title`);
  string(scenario.intro, `scenario ${scenario.id}.intro`);
  ids(scenario.choices, `scenario ${scenario.id}.choices`);
  scenario.choices.forEach((choice) => {
    string(choice.label, `choice ${choice.id}.label`);
    steps(choice.steps, `choice ${choice.id}.steps`);
  });
});
ids(data.topics, 'topics');
data.topics.forEach((topic) => {
  ['title', 'summary'].forEach((key) => string(topic[key], `topic ${topic.id}.${key}`));
  strings(topic.details, `topic ${topic.id}.details`);
  refs(topic.nodes, nodeIds, `topic ${topic.id}.nodes`);
  refs(topic.evidence, evidenceIds, `topic ${topic.id}.evidence`);
});
const sourceCache = new Map();
data.evidence.forEach((item) => {
  string(item.path, `evidence ${item.id}.path`);
  string(item.label, `evidence ${item.id}.label`);
  if (item.path.startsWith('/') || item.path.includes('\\') || item.path.split('/').some((part) => !part || part === '.' || part === '..')) fail(`unsafe evidence path ${item.path}`);
  if (!Number.isInteger(item.start) || !Number.isInteger(item.end) || item.start < 1 || item.end < item.start || item.end - item.start >= 80) fail(`evidence ${item.id}: expected a valid excerpt of at most 80 lines`);
  if (!sourceCache.has(item.path)) {
    const source = execFileSync('git', ['show', `${baseline}:${item.path}`], { cwd: root, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024 });
    const lines = source.split('\n');
    if (lines.at(-1) === '') lines.pop();
    sourceCache.set(item.path, lines);
  }
  const lines = sourceCache.get(item.path);
  if (item.end > lines.length) fail(`evidence ${item.id}: line ${item.end} exceeds ${item.path} (${lines.length} lines)`);
  item.text = lines.slice(item.start - 1, item.end).join('\n');
  item.url = `${github}/blob/${baseline}/${item.path.split('/').map(encodeURIComponent).join('/')}#L${item.start}-L${item.end}`;
});
const json = JSON.stringify(data).replace(/[<>&\u2028\u2029]/g, (character) => `\\u${character.charCodeAt(0).toString(16).padStart(4, '0')}`);
let output = read('index.template.html');
for (const [marker, value] of [['/* EXPLORER_STYLES */', read('styles.css')], ['/* EXPLORER_DATA */', json], ['/* EXPLORER_APP */', read('app.js')]]) {
  if (output.split(marker).length !== 2) throw new Error(`Template must contain exactly one ${marker}`);
  output = output.replace(marker, () => value);
}
const target = path.join(directory, 'index.html');
if (args.includes('--check')) {
  let existing;
  try { existing = readFileSync(target, 'utf8'); } catch { throw new Error('index.html is missing; run the build first'); }
  if (existing !== output) throw new Error('index.html is stale; run node docs/architecture-explorer/build.mjs');
  console.log(`Architecture explorer is current (${data.nodes.length} nodes, ${data.evidence.length} source excerpts).`);
} else {
  writeFileSync(target, output);
  console.log(`Built docs/architecture-explorer/index.html (${data.nodes.length} nodes, ${data.evidence.length} source excerpts).`);
}
