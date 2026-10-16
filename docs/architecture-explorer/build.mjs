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
const architecture = JSON.parse(read('architecture.json'));
const glossary = JSON.parse(read('glossary.json'));
data.architecture = architecture;
data.evidence = [...data.evidence, ...architecture.evidence];
data.terms = [...glossary, ...architecture.terms];
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
// Validate every graph before extracting immutable source excerpts.
object(architecture.entries, 'architecture.entries');
const viewIds = ids(architecture.views, 'architecture.views');
const views = new Map(architecture.views.map((view) => [view.id, view]));
if (Object.keys(architecture.entries).length !== nodeIds.size || Object.keys(architecture.entries).some((id) => !nodeIds.has(id))) fail('architecture.entries must cover overview nodes exactly');
for (const [owner, entry] of Object.entries(architecture.entries)) {
  if (!viewIds.has(entry)) fail(`unknown entry ${entry}`);
  if (views.get(entry).owner !== owner || views.get(entry).parent !== 'overview') fail(`entry ${entry} must belong to ${owner} under overview`);
}
architecture.views.forEach((view) => {
  ['title', 'summary'].forEach((key) => string(view[key], `view ${view.id}.${key}`));
  if (!nodeIds.has(view.owner)) fail(`view ${view.id}: unknown owner ${view.owner}`);
  if (view.parent !== 'overview' && !viewIds.has(view.parent)) fail(`view ${view.id}: unknown parent ${view.parent}`);
  if (view.parent === 'overview' && architecture.entries[view.owner] !== view.id) fail(`view ${view.id}: unregistered overview entry`);
  if (view.parent !== 'overview' && views.get(view.parent).owner !== view.owner) fail(`view ${view.id}: parent has a different owner`);
  const localIds = ids(view.nodes, `view ${view.id}.nodes`);
  view.nodes.forEach((node) => {
    ['label', 'layer', 'summary'].forEach((key) => string(node[key], `node ${node.id}.${key}`));
    strings(node.owns, `node ${node.id}.owns`);
    strings(node.boundaries, `node ${node.id}.boundaries`);
    refs(node.evidence, evidenceIds, `node ${node.id}.evidence`);
    if (node.target !== undefined && !viewIds.has(node.target)) fail(`node ${node.id}: unknown target ${node.target}`);
    if (node.external !== undefined && typeof node.external !== 'boolean') fail(`node ${node.id}: external must be boolean`);
    if (node.external && (!node.target || views.get(node.target).owner === view.owner || architecture.entries[views.get(node.target).owner] !== node.target)) fail(`node ${node.id}: external target must be another module entry`);
    if (node.target && !node.external && (views.get(node.target).parent !== view.id || views.get(node.target).owner !== view.owner)) fail(`node ${node.id}: internal target must be a child graph`);
  });
  array(view.relations, `view ${view.id}.relations`, true);
  const seen = new Set();
  view.relations.forEach((relation) => {
    object(relation, `view ${view.id}.relation`);
    refs([relation.from, relation.to], localIds, `view ${view.id}.relation endpoints`);
    if (!['call', 'data', 'dependency'].includes(relation.kind)) fail(`unknown relation kind ${relation.kind}`);
    string(relation.label, `view ${view.id}.relation.label`);
    const key = JSON.stringify(relation);
    if (seen.has(key)) fail(`view ${view.id}: duplicate relation`);
    seen.add(key);
  });
  const ancestors = new Set([view.id]);
  let parent = view.parent;
  while (parent !== 'overview') {
    if (ancestors.has(parent)) fail(`view ${view.id}: cyclic parents`);
    ancestors.add(parent);
    parent = views.get(parent)?.parent;
    if (!parent) fail(`view ${view.id}: unreachable parent`);
  }
  if (view.parent !== 'overview' && !views.get(view.parent).nodes.some((node) => node.target === view.id && !node.external)) fail(`view ${view.id}: parent has no drilldown link`);
});
const termIds = ids(data.terms, 'terms');
data.terms.forEach((term) => {
  string(term.label, `term ${term.id}.label`);
  string(term.explanation, `term ${term.id}.explanation`);
  if (term.label.includes('{{') || term.label.includes('}}')) fail(`term ${term.id}: label must be plain English text`);
  if (/[\u3400-\u9fff]/.test(term.label)) fail(`term ${term.id}: label must be English`);
  if (term.explanation.length < 20 || !/[\u3400-\u9fff]/.test(term.explanation)) fail(`term ${term.id}: explanation must describe a concrete situation in Chinese`);
});
const validateText = (value, name) => {
  if (typeof value === 'string') {
    const remainder = value.replace(/\{\{([a-z][a-z0-9-]*)\}\}/g, (_, id) => {
      if (!termIds.has(id)) fail(`${name}: unknown term ${id}`);
      return '';
    });
    if (remainder.includes('{{') || remainder.includes('}}')) fail(`${name}: malformed term marker`);
  } else if (Array.isArray(value)) value.forEach((item, index) => validateText(item, `${name}[${index}]`));
  else if (value && typeof value === 'object') Object.entries(value).forEach(([key, item]) => {
    if (!['id', 'from', 'to', 'kind', 'target', 'parent', 'owner', 'external', 'evidence', 'active', 'nodes', 'entries', 'path', 'text', 'url', 'baseline'].includes(key)) validateText(item, `${name}.${key}`);
    else if (key === 'nodes' && Array.isArray(item) && typeof item[0] === 'object') validateText(item, `${name}.${key}`);
  });
};
validateText(data, 'content');
data.evidence.forEach((item) => validateText(item.label, `evidence ${item.id}.label`));
// Static copy can opt into the same explicit term syntax.
validateText(read('index.template.html'), 'index.template.html');
// App source also contains parser syntax such as includes('{{'); inspect only authored markers.
const appCopy = read('app.js').replace(/\{\{([^{}\n]*)\}\}/g, (marker, id) => {
  if (!/^[a-z][a-z0-9-]*$/.test(id)) fail(`app.js: malformed term marker ${marker}`);
  if (!termIds.has(id)) fail(`app.js: unknown term ${id}`);
  return '';
});
if (/\{\{[a-z]/.test(appCopy) || /[a-z]\}\}/.test(appCopy)) fail('app.js: incomplete term marker');
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
