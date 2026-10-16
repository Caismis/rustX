(() => {
  'use strict';
  const data = JSON.parse(document.getElementById('explorer-data').textContent);
  const workspace = document.getElementById('workspace');
  const nodes = new Map(data.nodes.map((node) => [node.id, node]));
  const evidence = new Map(data.evidence.map((item) => [item.id, item]));
  const layers = [...new Set(data.nodes.map((node) => node.layer))];
  const state = { mode: 'map', node: data.nodes[0].id, relation: 'all', request: 0, scenario: 0, choice: 0, step: 0, topic: 0 };
  const escape = (value) => String(value).replace(/[&<>"']/g, (character) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[character]);
  const list = (items) => `<ul class="plain-list">${items.map((item) => `<li>${escape(item)}</li>`).join('')}</ul>`;
  const badge = (kind) => `<span class="relation-kind ${kind}">${{ call: '运行时调用', data: '数据流', dependency: '代码依赖' }[kind]}</span>`;
  const sourceDetails = (ids) => `<section class="source-section" aria-label="源码依据"><h3>查看源码 <span class="count">${ids.length}</span></h3><p class="source-help">源码摘录已打包，可离线查看。GitHub 链接会打开固定基线。</p>${ids.map((id) => {
    const item = evidence.get(id);
    return `<details class="source"><summary><span>${escape(item.label)}</span><code>${escape(item.path)}:${item.start}–${item.end}</code></summary><div class="source-body"><div class="source-toolbar"><span>基线 ${escape(data.baseline.commit.slice(0, 8))}</span><a href="${escape(item.url)}" target="_blank" rel="noopener noreferrer">在 GitHub 查看 ↗</a></div><pre tabindex="0" lang="en" aria-label="${escape(item.path)}，第 ${item.start} 至 ${item.end} 行"><code>${item.text.split('\n').map((line, index) => `<span class="source-line"><span class="line-number" aria-hidden="true">${item.start + index}</span><span>${escape(line) || ' '}</span></span>`).join('')}</code></pre></div></details>`;
  }).join('')}</section>`;
  const nodeChips = (ids) => `<div class="node-chips">${ids.map((id) => `<button type="button" class="node-chip" data-inspect="${escape(id)}">${escape(nodes.get(id).label)} <span aria-hidden="true">↗</span></button>`).join('')}</div>`;
  function mapMarkup(active = [], compact = false) {
    const selectedRelations = data.relations.filter((relation) => (relation.from === state.node || relation.to === state.node) && (state.relation === 'all' || relation.kind === state.relation));
    const neighbors = new Set(selectedRelations.flatMap((relation) => [relation.from, relation.to]));
    return `<div class="architecture-map ${compact ? 'compact' : ''}" data-map><svg class="map-lines" aria-hidden="true"><defs><marker id="arrow-call" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M 0 0 L 8 4 L 0 8 z"/></marker><marker id="arrow-data" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M 0 0 L 8 4 L 0 8 z"/></marker><marker id="arrow-dependency" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M 0 0 L 8 4 L 0 8 z"/></marker></defs><g data-lines></g></svg>${layers.map((layer, index) => `<section class="map-layer"><h3><span>${String(index + 1).padStart(2, '0')}</span>${escape(layer)}</h3><div class="layer-nodes">${data.nodes.filter((node) => node.layer === layer).map((node) => `<button type="button" data-node="${escape(node.id)}" class="map-node ${!compact && node.id === state.node ? 'selected' : ''} ${compact && active.includes(node.id) ? 'active' : ''} ${!compact && neighbors.has(node.id) && node.id !== state.node ? 'neighbor' : ''}" ${!compact ? `aria-pressed="${node.id === state.node}"` : `aria-label="查看 ${escape(node.label)}${active.includes(node.id) ? '，在此步骤中活跃' : ''}"`}><span class="node-name">${escape(node.label)}</span>${!compact ? `<span class="node-summary">${escape(node.summary)}</span>` : ''}${compact && active.includes(node.id) ? '<span class="active-dot" aria-hidden="true"></span>' : ''}</button>`).join('')}</div></section>`).join('')}</div>`;
  }
  function relationsMarkup(node) {
    const relations = data.relations.filter((relation) => (relation.from === node.id || relation.to === node.id) && (state.relation === 'all' || relation.kind === state.relation));
    return `<section class="relationships"><h3>关联职责</h3><p class="source-help">箭头表示关系方向。代码依赖不代表运行时调用。</p>${relations.length ? `<ul>${relations.map((relation) => `<li>${badge(relation.kind)}<span class="relation-endpoints"><button type="button" data-node="${escape(relation.from)}">${escape(nodes.get(relation.from).label)}</button><span aria-hidden="true">→</span><button type="button" data-node="${escape(relation.to)}">${escape(nodes.get(relation.to).label)}</button></span><p>${escape(relation.label)}</p></li>`).join('')}</ul>` : '<p>此职责没有这种类型的关联。</p>'}</section>`;
  }
  function renderMap() {
    const node = nodes.get(state.node);
    workspace.innerHTML = `<div class="section-heading"><div><p class="eyebrow">01 / 职责与关系</p><h2>按所有权理解架构。</h2></div><p>选择一项职责，查看它负责什么、边界在哪里，以及对应的源码依据。</p></div><div class="map-controls" role="group" aria-label="关系类型">${[['all', '所有关系'], ['call', '运行时调用'], ['data', '数据流'], ['dependency', '代码依赖']].map(([value, label]) => `<button type="button" class="filter ${value}" data-relation="${value}" aria-pressed="${state.relation === value}"><span aria-hidden="true"></span>${label}</button>`).join('')}</div><div class="map-layout"><div class="map-board"><div class="board-label"><span>职责地图</span><span>${data.nodes.length} 项职责 / ${layers.length} 个分组</span></div>${mapMarkup()}<p class="map-caption">选择节点以追踪关联。每张卡片代表一项架构职责，不一定对应独立进程。</p></div><aside class="inspector" aria-label="所选职责"><p class="eyebrow">${escape(node.layer)}</p><h2>${escape(node.label)}</h2><p class="lead">${escape(node.summary)}</p><h3>负责内容</h3>${list(node.owns)}<h3>职责边界</h3>${list(node.boundaries)}${relationsMarkup(node)}${sourceDetails(node.evidence)}</aside></div>`;
    drawLines();
  }
  function stepperMarkup(steps, index, context) {
    const step = steps[index];
    return `<div class="walk-layout"><aside class="step-index" aria-label="步骤导航"><p class="eyebrow">${context === 'request' ? '请求路径' : '结果路径'}</p><ol>${steps.map((item, itemIndex) => `<li><button type="button" data-step="${itemIndex}" ${index === itemIndex ? 'aria-current="step"' : ''}><span class="step-number">${String(itemIndex + 1).padStart(2, '0')}</span><span>${escape(item.title)}</span>${itemIndex < index ? '<span class="completed" aria-label="已浏览">✓</span>' : ''}</button></li>`).join('')}</ol></aside><article class="step-content"><div class="step-control"><span class="step-progress">第 ${index + 1} 步 / 共 ${steps.length} 步</span><div><button type="button" class="text-button" data-replay ${index === 0 ? 'disabled' : ''}>↺ 重置</button><button type="button" class="square-button" data-prev ${index === 0 ? 'disabled' : ''} aria-label="上一步">←</button><button type="button" class="square-button" data-next ${index === steps.length - 1 ? 'disabled' : ''} aria-label="下一步">→</button></div></div><div class="step-announcement" aria-live="polite" aria-atomic="true"><p class="eyebrow">执行方 / ${escape(step.actor)}</p><h2>${escape(step.title)}</h2></div><p class="step-description">${escape(step.description)}</p><div class="fact-grid"><section><h3><span aria-hidden="true">▣</span> 持久化事实</h3><p>${escape(step.durable)}</p></section><section><h3><span aria-hidden="true">◉</span> 客户端可见内容</h3><p>${escape(step.visible)}</p></section></div><section class="active-responsibilities"><h3>此步骤涉及的职责</h3>${nodeChips(step.active)}<p class="source-help">下方会突出显示相关职责。选择一项职责可查看其架构。</p>${mapMarkup(step.active, true)}</section>${sourceDetails(step.evidence)}<div class="bottom-step-control"><button type="button" class="secondary-button" data-prev ${index === 0 ? 'disabled' : ''}>← 上一步</button>${index < steps.length - 1 ? '<button type="button" class="primary-button" data-next>下一步 →</button>' : '<span class="path-complete">路径结束 <span aria-hidden="true">✓</span></span>'}</div></article></div>`;
  }
  function renderRequest() {
    workspace.innerHTML = `<div class="section-heading"><div><p class="eyebrow">02 / 从意图到观测</p><h2>追踪一次请求。</h2></div><p>沿着一条引导式执行路径，按自己的节奏前进，观察职责如何在系统中流转。</p></div>${stepperMarkup(data.request, state.request, 'request')}`;
  }
  function renderScenarios() {
    const scenario = data.scenarios[state.scenario];
    const choice = scenario.choices[state.choice];
    workspace.innerHTML = `<div class="section-heading"><div><p class="eyebrow">03 / 边界上的决策</p><h2>遇到这些情况时……</h2></div><p>选择一种情境和结果。这些有源码依据的路径说明了架构的约束。</p></div><div class="scenario-tabs" role="group" aria-label="边界场景">${data.scenarios.map((item, index) => `<button type="button" data-scenario="${index}" aria-pressed="${index === state.scenario}"><span>${String(index + 1).padStart(2, '0')}</span>${escape(item.title)}</button>`).join('')}</div><section class="scenario-intro"><div><h2>${escape(scenario.title)}</h2><p>${escape(scenario.intro)}</p></div><fieldset><legend>选择一种结果</legend>${scenario.choices.map((item, index) => `<label class="choice ${state.choice === index ? 'chosen' : ''}"><input type="radio" name="outcome" data-choice="${index}" value="${index}" ${state.choice === index ? 'checked' : ''}><span>${escape(item.label)}</span></label>`).join('')}</fieldset></section>${stepperMarkup(choice.steps, state.step, 'scenario')}`;
  }
  function renderTopics() {
    const topic = data.topics[state.topic];
    workspace.innerHTML = `<div class="section-heading"><div><p class="eyebrow">04 / 深入一层</p><h2>理解系统如何协同运作。</h2></div><p>了解一项设计决策，追踪相关职责，并查看对应的源码快照。</p></div><div class="topic-layout"><aside class="topic-nav" aria-label="深入专题">${data.topics.map((item, index) => `<button type="button" data-topic="${index}" ${index === state.topic ? 'aria-current="true"' : ''}><span>${String(index + 1).padStart(2, '0')}</span>${escape(item.title)}<span aria-hidden="true">↗</span></button>`).join('')}</aside><article class="topic-content"><p class="eyebrow">深入专题 / ${String(state.topic + 1).padStart(2, '0')}</p><h2>${escape(topic.title)}</h2><p class="lead">${escape(topic.summary)}</p><div class="topic-details">${topic.details.map((detail) => `<p>${escape(detail)}</p>`).join('')}</div><h3>相关职责</h3>${nodeChips(topic.nodes)}${sourceDetails(topic.evidence)}</article></div>`;
  }
  function render() {
    document.querySelectorAll('[data-mode]').forEach((button) => {
      if (button.dataset.mode === state.mode) button.setAttribute('aria-current', 'page');
      else button.removeAttribute('aria-current');
    });
    ({ map: renderMap, request: renderRequest, scenarios: renderScenarios, topics: renderTopics })[state.mode]();
    revealCurrentStep();
  }
  function revealCurrentStep() {
    const strip = workspace.querySelector('.step-index ol');
    const current = strip?.querySelector('[aria-current="step"]');
    if (!current || strip.scrollWidth <= strip.clientWidth) return;
    const stripBounds = strip.getBoundingClientRect();
    const currentBounds = current.getBoundingClientRect();
    if (currentBounds.right > stripBounds.right - 4) {
      strip.scrollLeft += currentBounds.right - stripBounds.right + 4;
    } else if (currentBounds.left < stripBounds.left + 4) {
      strip.scrollLeft += currentBounds.left - stripBounds.left - 4;
    }
  }
  function drawLines() {
    const map = workspace.querySelector('[data-map]:not(.compact)');
    if (!map) return;
    const svg = map.querySelector('svg');
    const group = svg.querySelector('[data-lines]');
    const bounds = map.getBoundingClientRect();
    svg.setAttribute('viewBox', `0 0 ${bounds.width} ${bounds.height}`);
    const relations = data.relations.filter((relation) => (relation.from === state.node || relation.to === state.node) && (state.relation === 'all' || relation.kind === state.relation));
    group.replaceChildren();
    relations.forEach((relation) => {
      const from = map.querySelector(`[data-node="${relation.from}"]`).getBoundingClientRect();
      const to = map.querySelector(`[data-node="${relation.to}"]`).getBoundingClientRect();
      const down = to.top >= from.top;
      const sameRow = Math.abs(from.top - to.top) < Math.min(from.height, to.height) / 2;
      const right = to.left >= from.left;
      const x1 = (sameRow ? (right ? from.right : from.left) : from.left + from.width / 2) - bounds.left;
      const y1 = (sameRow ? from.top + from.height / 2 : down ? from.bottom : from.top) - bounds.top;
      const x2 = (sameRow ? (right ? to.left : to.right) : to.left + to.width / 2) - bounds.left;
      const y2 = (sameRow ? to.top + to.height / 2 : down ? to.top : to.bottom) - bounds.top;
      const curve = sameRow ? `M${x1},${y1} C${(x1 + x2) / 2},${y1} ${(x1 + x2) / 2},${y2} ${x2},${y2}` : `M${x1},${y1} C${x1},${(y1 + y2) / 2} ${x2},${(y1 + y2) / 2} ${x2},${y2}`;
      const line = document.createElementNS('http://www.w3.org/2000/svg', 'path');
      line.setAttribute('d', curve);
      line.setAttribute('class', `map-line ${relation.kind}`);
      line.setAttribute('marker-end', `url(#arrow-${relation.kind})`);
      group.append(line);
    });
  }
  function navigateNode(id) {
    state.node = id;
    state.mode = 'map';
    render();
    workspace.querySelector('.inspector').scrollIntoView({ block: 'nearest' });
    const heading = workspace.querySelector('.inspector h2');
    heading.tabIndex = -1;
    heading.focus({ preventScroll: true });
  }
  document.addEventListener('click', (event) => {
    const button = event.target.closest('button');
    if (!button || button.disabled) return;
    if (button.dataset.mode) { state.mode = button.dataset.mode; render(); return; }
    if (button.dataset.inspect) { navigateNode(button.dataset.inspect); return; }
    if (button.dataset.node) {
      if (state.mode !== 'map') { navigateNode(button.dataset.node); return; }
      state.node = button.dataset.node; render();
      if (window.matchMedia('(max-width: 850px)').matches) {
        const heading = workspace.querySelector('.inspector h2');
        heading.tabIndex = -1;
        heading.focus({ preventScroll: true });
        heading.scrollIntoView({ block: 'nearest' });
      } else {
        workspace.querySelector(`.map-node[data-node="${state.node}"]`).focus({ preventScroll: true });
      }
      return;
    }
    if (button.dataset.relation) {
      state.relation = button.dataset.relation; render();
      workspace.querySelector(`[data-relation="${state.relation}"]`).focus({ preventScroll: true });
      return;
    }
    if (button.dataset.scenario !== undefined) { state.scenario = Number(button.dataset.scenario); state.choice = 0; state.step = 0; render(); workspace.querySelector(`[data-scenario="${state.scenario}"]`).focus({ preventScroll: true }); return; }
    if (button.dataset.topic !== undefined) { state.topic = Number(button.dataset.topic); render(); workspace.querySelector(`[data-topic="${state.topic}"]`).focus({ preventScroll: true }); return; }
    const indexKey = state.mode === 'request' ? 'request' : 'step';
    const steps = state.mode === 'request' ? data.request : data.scenarios[state.scenario].choices[state.choice].steps;
    let changed = false;
    if (button.dataset.step !== undefined) { state[indexKey] = Number(button.dataset.step); changed = true; }
    if (button.hasAttribute('data-prev')) { state[indexKey] = Math.max(0, state[indexKey] - 1); changed = true; }
    if (button.hasAttribute('data-next')) { state[indexKey] = Math.min(steps.length - 1, state[indexKey] + 1); changed = true; }
    if (button.hasAttribute('data-replay')) { state[indexKey] = 0; changed = true; }
    if (changed) {
      render();
      const heading = workspace.querySelector('.step-announcement h2');
      heading.tabIndex = -1;
      heading.focus({ preventScroll: true });
      heading.scrollIntoView({ block: 'nearest' });
    }
  });
  document.addEventListener('change', (event) => {
    if (event.target.name === 'outcome') {
      state.choice = Number(event.target.value); state.step = 0; render();
      workspace.querySelector(`input[name="outcome"][value="${state.choice}"]`).focus({ preventScroll: true });
    }
  });
  document.querySelector('.wordmark').addEventListener('click', (event) => {
    event.preventDefault(); state.mode = 'map'; render(); window.scrollTo({ top: 0 });
  });
  document.getElementById('intro').textContent = data.intro;
  document.title = `${data.title} · rustX`;
  document.getElementById('baseline').innerHTML = `<span class="baseline-tag">源码基线</span><details class="baseline-details"><summary><code>${escape(data.baseline.commit.slice(0, 8))}</code> <span>${escape(data.baseline.branch)}</span><span aria-hidden="true">⌄</span></summary><div><code>${escape(data.baseline.commit)}</code><p lang="en">${escape(data.baseline.subject)}</p><p>本导览介绍的是固定快照，不会跟踪当前工作区，也不会执行请求。</p></div></details><span class="simulation-label">教学模拟</span>`;
  render();
  new ResizeObserver(() => {
    drawLines();
    revealCurrentStep();
  }).observe(workspace);
})();
