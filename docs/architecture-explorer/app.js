(() => {
  'use strict';
  const data = JSON.parse(document.getElementById('explorer-data').textContent);
  const workspace = document.getElementById('workspace');
  const nodes = new Map(data.nodes.map((node) => [node.id, node]));
  const evidence = new Map(data.evidence.map((item) => [item.id, item]));
  const views = new Map(data.architecture.views.map((view) => [view.id, view]));
  const terms = new Map(data.terms.map((term) => [term.id, term]));
  const history = [];
  const memories = new Map();
  const state = { mode: 'map', view: 'overview', node: data.nodes[0].id, relation: 'all', request: 0, scenario: 0, choice: 0, step: 0, topic: 0 };
  const escape = (value) => String(value).replace(/[&<>"']/g, (character) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[character]);
  const plain = (value) => String(value).replace(/\{\{([a-z][a-z0-9-]*)\}\}/g, (_, id) => terms.get(id).label);
  const rich = (value) => String(value).split(/(\{\{[a-z][a-z0-9-]*\}\})/g).map((part) => {
    const match = /^\{\{([a-z][a-z0-9-]*)\}\}$/.exec(part);
    if (!match) return escape(part);
    const term = terms.get(match[1]);
    return `<span class="term" data-term="${term.id}" lang="en">${escape(term.label)}</span>`;
  }).join('');
  const list = (items) => `<ul class="plain-list">${items.map((item) => `<li>${rich(item)}</li>`).join('')}</ul>`;
  const badge = (kind) => `<span class="relation-kind ${kind}">${rich({ call: '运行时调用', data: '数据流', dependency: '代码依赖' }[kind])}</span>`;
  const graph = () => state.view === 'overview' ? { title: '架构总览', summary: '先了解各模块如何协作，再进入内部，逐层查看实现。', nodes: data.nodes, relations: data.relations } : views.get(state.view);
  const targetOf = (node) => state.view === 'overview' ? data.architecture.entries[node.id] : node.target;
  const visibleRelations = () => state.view === 'overview' ? selectedRelations() : graph().relations.filter((relation) => state.relation === 'all' || relation.kind === state.relation);
  const selectedRelations = () => graph().relations.filter((relation) => (relation.from === state.node || relation.to === state.node) && (state.relation === 'all' || relation.kind === state.relation));
  const sourceDetails = (ids) => `<section class="source-section" aria-label="源码依据"><h3>查看 {{source-code}} <span class="count">${ids.length}</span></h3><p class="source-help">{{source-code}} 摘录已打包，可离线查看。GitHub 链接会打开固定基线。</p>${ids.map((id) => {
    const item = evidence.get(id);
    return `<details class="source" data-source="${escape(id)}"><summary><span>${rich(item.label)}</span><code>${escape(item.path)}:${item.start}–${item.end}</code></summary><div class="source-body"><div class="source-toolbar"><span>基线 ${escape(data.baseline.commit.slice(0, 8))}</span><a href="${escape(item.url)}" target="_blank" rel="noopener noreferrer">在 GitHub 查看 ↗</a></div><pre tabindex="0" lang="en" aria-label="${escape(item.path)}，第 ${item.start} 至 ${item.end} 行"><code>${item.text.split('\n').map((line, index) => `<span class="source-line"><span class="line-number" aria-hidden="true">${item.start + index}</span><span>${escape(line) || ' '}</span></span>`).join('')}</code></pre></div></details>`;
  }).join('')}</section>`;
  const nodeChips = (ids) => `<div class="node-chips">${ids.map((id) => `<button type="button" class="node-chip" data-inspect="${escape(id)}">${rich(nodes.get(id).label)} <span aria-hidden="true">↗</span></button>`).join('')}</div>`;
  function mapMarkup(active = [], compact = false) {
    const current = compact ? { nodes: data.nodes } : graph();
    const groups = [...new Set(current.nodes.map((node) => node.layer))];
    const neighbors = new Set(compact ? [] : selectedRelations().flatMap((relation) => [relation.from, relation.to]));
    const markers = ['call', 'data', 'dependency'].map((kind) => `<marker id="arrow-${kind}" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M 0 0 L 8 4 L 0 8 z"/></marker>`).join('');
    return `<div class="architecture-map ${compact ? 'compact' : state.view !== 'overview' ? 'detail-graph' : ''}" data-map>${compact ? '' : `<svg class="map-lines" aria-hidden="true"><defs>${markers}</defs><g data-lines></g></svg>`}${groups.map((layer, index) => `<section class="map-layer"><h3><span>${String(index + 1).padStart(2, '0')}</span>${rich(layer)}</h3><div class="layer-nodes">${current.nodes.filter((node) => node.layer === layer).map((node) => {
      const classes = `${!compact && node.id === state.node ? 'selected' : ''} ${compact && active.includes(node.id) ? 'active' : ''} ${!compact && neighbors.has(node.id) && node.id !== state.node ? 'neighbor' : ''} ${node.external ? 'external-node' : ''}`;
      const selection = `<button type="button" data-node="${escape(node.id)}" class="map-node ${classes}" aria-label="${escape(plain(node.label))}，${compact ? '在架构总览中查看' : '选择并查看右侧说明'}" ${!compact ? `aria-pressed="${node.id === state.node}"` : ''}><span class="node-name">${rich(node.label)}</span>${!compact ? `<span class="node-summary">${rich(node.summary)}</span>` : ''}${compact && active.includes(node.id) ? '<span class="active-dot" aria-hidden="true"></span>' : ''}</button>`;
      if (compact) return selection;
      const target = targetOf(node);
      const action = target ? `<button type="button" class="node-enter" data-enter="${escape(target)}" aria-label="进入 ${escape(plain(node.label))} 内部">进入内部 <span aria-hidden="true">${node.external ? '↗' : '→'}</span></button>` : '<span class="node-leaf">查看实现</span>';
      return `<div class="node-card ${classes}">${selection}<div class="node-action">${node.external ? `<span class="external-owner">${rich('外部 {{owner}}')}</span>` : ''}${action}</div></div>`;
    }).join('')}</div></section>`).join('')}</div>`;
  }
  function relationsMarkup(node) {
    const relations = selectedRelations();
    const local = new Map(graph().nodes.map((item) => [item.id, item]));
    return `<section class="relationships"><h3>相关关系</h3><p class="source-help">箭头表示关系方向。代码依赖不代表运行时调用。</p>${relations.length ? `<ul>${relations.map((relation) => `<li>${badge(relation.kind)}<span class="relation-endpoints"><button type="button" data-select="${escape(relation.from)}">${rich(local.get(relation.from).label)}</button><span aria-hidden="true">→</span><button type="button" data-select="${escape(relation.to)}">${rich(local.get(relation.to).label)}</button></span><p>${rich(relation.label)}</p></li>`).join('')}</ul>` : '<p>此节点没有这种类型的关联。</p>'}</section>`;
  }
  function breadcrumbs() {
    const path = [];
    let current = state.view;
    while (current !== 'overview') { path.unshift(views.get(current)); current = views.get(current).parent; }
    return `<nav class="graph-navigation" aria-label="图层导航"><button type="button" class="secondary-button" data-back ${history.length ? '' : 'disabled'}>← 返回上一处</button><ol><li><button type="button" data-view="overview" ${state.view === 'overview' ? 'aria-current="page"' : ''}>架构总览</button></li>${path.map((view) => `<li><span aria-hidden="true">/</span><button type="button" data-view="${view.id}" ${view.id === state.view ? 'aria-current="page"' : ''}>${rich(view.title)}</button></li>`).join('')}</ol></nav>`;
  }
  function relationControls() {
    const options = [['all', '所有关系'], ['call', '运行时调用'], ['data', '数据流'], ['dependency', '代码依赖']];
    return `<div class="map-controls" role="group" aria-label="关系类型">${options.map(([value, label]) =>
      `<button type="button" class="filter ${value}" data-relation="${value}" aria-pressed="${state.relation === value}"><span aria-hidden="true"></span>${label}</button>`
    ).join('')}</div>`;
  }
  function inspectorMarkup(node) {
    const target = targetOf(node);
    const next = target
      ? `<button type="button" class="primary-button enter-graph" data-enter="${target}">${node.external ? '进入内部 ↗' : '进入内部 →'}</button>`
      : `<p class="leaf-note">${rich('此处为实现末端，可展开下方 {{source-code}} 依据。')}</p>`;
    return `<aside class="inspector" aria-label="所选节点">
      <p class="eyebrow">${rich(node.layer)}${node.external ? ' · 外部模块' : ''}</p>
      <h2>${rich(node.label)}</h2><p class="lead">${rich(node.summary)}</p>${next}
      <h3>负责什么</h3>${list(node.owns)}
      <h3>需要留意什么</h3>${list(node.boundaries)}
      ${relationsMarkup(node)}${sourceDetails(node.evidence)}
    </aside>`;
  }
  function renderMap() {
    const current = graph();
    const node = current.nodes.find((item) => item.id === state.node);
    const groupCount = new Set(current.nodes.map((item) => item.layer)).size;
    const caption = '选择卡片主体查看右侧说明，点击“进入内部”继续探索。将鼠标停在带点状下划线的术语上，可查看该词的解释。实现末端可展开 {{source-code}}。虚线外框表示由其他模块负责的节点。' + (state.view === 'overview' ? '总览显示选中节点的关系。' : '内部图显示当前过滤下的所有关系，粗线突出选中节点的关系。') + '右侧清单解释选中节点的关系。';
    workspace.innerHTML = `${breadcrumbs()}
      <div class="section-heading">
        <div><p class="eyebrow">01 / ${state.view === 'overview' ? '模块总览' : '模块内部'}</p>
        <h2 tabindex="-1" data-graph-heading>${rich(current.title)}</h2></div><p>${rich(current.summary)}</p>
      </div>
      ${relationControls()}
      <div class="map-layout"><div class="map-board">
        <div class="board-label"><span>${state.view === 'overview' ? '模块入口' : '内部结构'}</span><span>${current.nodes.length} 个节点 / ${groupCount} 个分组</span></div>
        ${mapMarkup()}<p class="map-caption">${rich(caption)}</p>
      </div>${inspectorMarkup(node)}</div>`;
    drawLines();
  }
  function stepperMarkup(steps, index, context) {
    const step = steps[index];
    return `<div class="walk-layout"><aside class="step-index" aria-label="步骤导航"><p class="eyebrow">${context === 'request' ? '请求路径' : '结果路径'}</p><ol>${steps.map((item, itemIndex) => `<li><button type="button" data-step="${itemIndex}" ${index === itemIndex ? 'aria-current="step"' : ''}><span class="step-number">${String(itemIndex + 1).padStart(2, '0')}</span><span>${rich(item.title)}</span>${itemIndex < index ? '<span class="completed" aria-label="已浏览">✓</span>' : ''}</button></li>`).join('')}</ol></aside><article class="step-content"><div class="step-control"><span class="step-progress">第 ${index + 1} 步 / 共 ${steps.length} 步</span><div><button type="button" class="text-button" data-replay ${index === 0 ? 'disabled' : ''}>↺ 重置</button><button type="button" class="square-button" data-prev ${index === 0 ? 'disabled' : ''} aria-label="上一步">←</button><button type="button" class="square-button" data-next ${index === steps.length - 1 ? 'disabled' : ''} aria-label="下一步">→</button></div></div><div class="step-announcement" aria-live="polite" aria-atomic="true"><p class="eyebrow">执行方 / ${rich(step.actor)}</p><h2>${rich(step.title)}</h2></div><p class="step-description">${rich(step.description)}</p><div class="fact-grid"><section><h3><span aria-hidden="true">▣</span> 已保存的事实</h3><p>${rich(step.durable)}</p></section><section><h3><span aria-hidden="true">◉</span> 客户端可见内容</h3><p>${rich(step.visible)}</p></section></div><section class="active-responsibilities"><h3>这一步涉及哪些模块</h3>${nodeChips(step.active)}<p class="source-help">下方突出显示参与这一步的模块。选择模块可查看它在架构中的位置。</p>${mapMarkup(step.active, true)}</section>${sourceDetails(step.evidence)}<div class="bottom-step-control"><button type="button" class="secondary-button" data-prev ${index === 0 ? 'disabled' : ''}>← 上一步</button>${index < steps.length - 1 ? '<button type="button" class="primary-button" data-next>下一步 →</button>' : '<span class="path-complete">路径结束 <span aria-hidden="true">✓</span></span>'}</div></article></div>`;
  }
  function renderRequest() {
    workspace.innerHTML = `<div class="section-heading"><div><p class="eyebrow">02 / 一次请求的完整路径</p><h2>追踪一次请求。</h2></div><p>按自己的节奏逐步前进，看看一次请求如何经过各个模块，以及每一步会留下什么结果。</p></div>${stepperMarkup(data.request, state.request, 'request')}`;
  }
  function renderScenarios() {
    const scenario = data.scenarios[state.scenario];
    const choice = scenario.choices[state.choice];
    workspace.innerHTML = `<div class="section-heading"><div><p class="eyebrow">03 / 特殊情况如何处理</p><h2>遇到这些情况时……</h2></div><p>选择一种情境和结果。沿着有 {{source-code}} 依据的路径，看看系统如何处理这些情况。</p></div><div class="scenario-tabs" role="group" aria-label="特殊情况">${data.scenarios.map((item, index) => `<button type="button" data-scenario="${index}" aria-pressed="${index === state.scenario}"><span>${String(index + 1).padStart(2, '0')}</span>${rich(item.title)}</button>`).join('')}</div><section class="scenario-intro"><div><h2>${rich(scenario.title)}</h2><p>${rich(scenario.intro)}</p></div><fieldset><legend>选择一种结果</legend>${scenario.choices.map((item, index) => `<label class="choice ${state.choice === index ? 'chosen' : ''}"><input type="radio" name="outcome" data-choice="${index}" value="${index}" ${state.choice === index ? 'checked' : ''}><span>${rich(item.label)}</span></label>`).join('')}</fieldset></section>${stepperMarkup(choice.steps, state.step, 'scenario')}`;
  }
  function renderTopics() {
    const topic = data.topics[state.topic];
    workspace.innerHTML = `<div class="section-heading"><div><p class="eyebrow">04 / 深入一层</p><h2>理解系统如何协同运作。</h2></div><p>从一个设计问题出发，了解相关模块如何协作，并查看对应的 {{source-code}}。</p></div><div class="topic-layout"><aside class="topic-nav" aria-label="深入专题">${data.topics.map((item, index) => `<button type="button" data-topic="${index}" ${index === state.topic ? 'aria-current="true"' : ''}><span>${String(index + 1).padStart(2, '0')}</span>${rich(item.title)}<span aria-hidden="true">↗</span></button>`).join('')}</aside><article class="topic-content"><p class="eyebrow">深入专题 / ${String(state.topic + 1).padStart(2, '0')}</p><h2>${rich(topic.title)}</h2><p class="lead">${rich(topic.summary)}</p><div class="topic-details">${topic.details.map((detail) => `<p>${rich(detail)}</p>`).join('')}</div><h3>相关模块</h3>${nodeChips(topic.nodes)}${sourceDetails(topic.evidence)}</article></div>`;
  }
  function render() {
    document.querySelectorAll('[data-mode]').forEach((button) => {
      if (button.dataset.mode === state.mode) button.setAttribute('aria-current', 'page');
      else button.removeAttribute('aria-current');
    });
    hideTooltip();
    ({ map: renderMap, request: renderRequest, scenarios: renderScenarios, topics: renderTopics })[state.mode]();
    hydrateTerms();
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
    const relations = visibleRelations().sort((a, b) => Number(a.from === state.node || a.to === state.node) - Number(b.from === state.node || b.to === state.node));
    group.replaceChildren();
    relations.forEach((relation) => {
      const from = map.querySelector(`[data-node="${relation.from}"]`).closest('.node-card').getBoundingClientRect();
      const to = map.querySelector(`[data-node="${relation.to}"]`).closest('.node-card').getBoundingClientRect();
      const down = to.top >= from.top;
      const sameRow = Math.abs(from.top - to.top) < Math.min(from.height, to.height) / 2;
      const right = to.left >= from.left;
      const x1 = (sameRow ? (right ? from.right : from.left) : from.left + from.width / 2) - bounds.left;
      const y1 = (sameRow ? from.top + from.height / 2 : down ? from.bottom : from.top) - bounds.top;
      const x2 = (sameRow ? (right ? to.left : to.right) : to.left + to.width / 2) - bounds.left;
      const y2 = (sameRow ? to.top + to.height / 2 : down ? to.top : to.bottom) - bounds.top;
      let curve = sameRow ? `M${x1},${y1} C${(x1 + x2) / 2},${y1} ${(x1 + x2) / 2},${y2} ${x2},${y2}` : `M${x1},${y1} C${x1},${(y1 + y2) / 2} ${x2},${(y1 + y2) / 2} ${x2},${y2}`;
      if (state.view !== 'overview' && !sameRow) {
        // Use the space between columns or the outer gutters for longer local edges.
        const sameColumn = Math.abs(from.left - to.left) < Math.min(from.width, to.width) / 2;
        const leftColumn = from.left + from.width / 2 < bounds.left + bounds.width / 2;
        const startRight = sameColumn ? !leftColumn : right;
        const endRight = sameColumn ? !leftColumn : !right;
        const startX = (startRight ? from.right : from.left) - bounds.left;
        const endX = (endRight ? to.right : to.left) - bounds.left;
        const startY = from.top + from.height / 2 - bounds.top;
        const endY = to.top + to.height / 2 - bounds.top;
        const lane = sameColumn ? (leftColumn ? 1 : bounds.width - 1) : bounds.width / 2;
        curve = `M${startX},${startY} H${lane} V${endY} H${endX}`;
      }
      const line = document.createElementNS('http://www.w3.org/2000/svg', 'path');
      line.setAttribute('d', curve);
      line.setAttribute('class', `map-line ${relation.kind}${state.view === 'overview' ? '' : relation.from === state.node || relation.to === state.node ? ' emphasized' : ' subdued'}`);
      line.setAttribute('marker-end', `url(#arrow-${relation.kind})`);
      group.append(line);
    });
  }
  const routeKey = () => state.mode === 'map' ? `map:${state.view}` : state.mode;
  function capture() { return { state: { ...state }, x: window.scrollX, y: window.scrollY, disclosures: [...workspace.querySelectorAll('details[open]')].map((item) => item.dataset.source) }; }
  function remember() { memories.set(routeKey(), capture()); }
  function restore(saved, focus = false) {
    Object.assign(state, saved.state); render();
    saved.disclosures.forEach((id) => { const detail = workspace.querySelector(`details[data-source="${id}"]`); if (detail) detail.open = true; });
    window.scrollTo(saved.x, saved.y);
    if (focus) workspace.querySelector('[data-graph-heading]')?.focus({ preventScroll: true });
  }
  function openView(id) {
    if (state.mode === 'map' && state.view === id) return;
    remember(); history.push(capture());
    const saved = memories.get(`map:${id}`);
    if (saved) { restore(saved, true); return; }
    state.mode = 'map'; state.view = id; state.node = graph().nodes[0].id; state.relation = 'all';
    render(); workspace.scrollIntoView({ block: 'start' });
    workspace.querySelector('[data-graph-heading]').focus({ preventScroll: true });
  }
  function navigateNode(id) { openView('overview'); selectNode(id); }
  function selectNode(id) {
    state.node = id; render();
    workspace.querySelector(`.map-node[data-node="${id}"]`).focus({ preventScroll: true });
  }
  const tooltip = document.createElement('div');
  tooltip.id = 'term-tooltip'; tooltip.className = 'term-tooltip'; tooltip.setAttribute('role', 'tooltip'); tooltip.hidden = true;
  document.body.append(tooltip);
  let tooltipAnchor = null;
  let tooltipTimer;
  let hoverPaused = false;
  function hideTooltip() {
    clearTimeout(tooltipTimer);
    if (tooltipAnchor) tooltipAnchor.removeAttribute('aria-describedby');
    tooltipAnchor = null; tooltip.hidden = true;
  }
  function positionTooltip() {
    if (!tooltipAnchor || tooltip.hidden) return;
    const bounds = (tooltipAnchor.closest('.node-card') || tooltipAnchor.closest('button,summary,a,label') || tooltipAnchor).getBoundingClientRect();
    tooltip.style.maxHeight = 'calc(100vh - 24px)';
    const width = tooltip.offsetWidth; const height = tooltip.offsetHeight;
    const gap = 10; const margin = 12;
    let left = Math.max(margin, Math.min(bounds.left, window.innerWidth - width - margin));
    let top;
    if (bounds.bottom + gap + height <= window.innerHeight - margin) top = bounds.bottom + gap;
    else if (bounds.top - gap - height >= margin) top = bounds.top - gap - height;
    else if (bounds.right + gap + width <= window.innerWidth - margin) {
      left = bounds.right + gap; top = Math.max(margin, Math.min(bounds.top, window.innerHeight - height - margin));
    } else if (bounds.left - gap - width >= margin) {
      left = bounds.left - gap - width; top = Math.max(margin, Math.min(bounds.top, window.innerHeight - height - margin));
    } else {
      const below = window.innerHeight - bounds.bottom - gap - margin;
      const above = bounds.top - gap - margin;
      const available = Math.max(below, above, 40);
      tooltip.style.maxHeight = `${available}px`;
      top = below >= above ? bounds.bottom + gap : Math.max(margin, bounds.top - gap - tooltip.offsetHeight);
    }
    tooltip.style.left = `${left}px`; tooltip.style.top = `${top}px`;
  }
  function showTooltip(anchor) {
    hideTooltip(); tooltipAnchor = anchor;
    const term = terms.get(anchor.dataset.term);
    tooltip.innerHTML = `<p><strong lang="en">${escape(term.label)}</strong><span>${escape(plain(term.explanation))}</span></p>`;
    tooltip.hidden = false; anchor.setAttribute('aria-describedby', tooltip.id); positionTooltip();
  }
  function hydrateTerms() {
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    const pending = [];
    while (walker.nextNode()) {
      const node = walker.currentNode;
      if (node.textContent.includes('{{') && !node.parentElement.closest('script,style,pre,code,[data-verbatim],.term-tooltip')) pending.push(node);
    }
    pending.forEach((node) => {
      const template = document.createElement('template'); template.innerHTML = rich(node.textContent); node.replaceWith(template.content);
    });
    document.querySelectorAll('[aria-label]').forEach((element) => {
      const label = element.getAttribute('aria-label');
      if (label.includes('{{')) element.setAttribute('aria-label', plain(label));
    });
    document.querySelectorAll('.term:not(.term-tooltip .term)').forEach((span) => {
      if (!span.closest('button,a,summary,label')) span.tabIndex = 0;
    });
  }
  const delayedHide = () => { clearTimeout(tooltipTimer); tooltipTimer = setTimeout(() => {
    if (tooltipAnchor && (tooltipAnchor.matches(':hover') || tooltip.matches(':hover') || tooltipAnchor === document.activeElement || tooltipAnchor.contains(document.activeElement))) return;
    hideTooltip();
  }, 140); };
  document.addEventListener('pointerover', (event) => {
    if (hoverPaused) return;
    const span = event.target.closest('.term');
    if (span && !tooltip.contains(span)) showTooltip(span);
    else if (tooltip.contains(event.target)) clearTimeout(tooltipTimer);
  });
  document.addEventListener('pointermove', (event) => {
    if (!hoverPaused) return;
    hoverPaused = false;
    const span = event.target.closest('.term');
    if (span && !tooltip.contains(span)) showTooltip(span);
  });
  document.addEventListener('pointerout', (event) => {
    if (event.target.closest('.term') || tooltip.contains(event.target)) {
      if (tooltip.contains(event.relatedTarget) || tooltipAnchor?.contains(event.relatedTarget)) return;
      delayedHide();
    }
  });
  document.addEventListener('focusin', (event) => {
    const anchor = event.target;
    if (anchor.matches('.term')) showTooltip(anchor); else hideTooltip();
  });
  document.addEventListener('focusout', delayedHide);
  document.addEventListener('keydown', (event) => { if (event.key === 'Escape') hideTooltip(); });
  window.addEventListener('scroll', positionTooltip, { passive: true });
  window.addEventListener('resize', positionTooltip);
  document.addEventListener('click', (event) => {
    if (event.target.closest('button,summary,a,label,input')) {
      hoverPaused = true;
      hideTooltip();
    }
    const button = event.target.closest('button');
    if (!button || button.disabled) return;
    if (button.dataset.mode) {
      if (state.mode === button.dataset.mode) return;
      remember(); history.push(capture());
      const saved = memories.get(button.dataset.mode === 'map' ? `map:${state.view}` : button.dataset.mode);
      if (saved) restore(saved); else { state.mode = button.dataset.mode; render(); workspace.scrollIntoView({ block: 'start' }); }
      return;
    }
    if (button.hasAttribute('data-back')) { remember(); const saved = history.pop(); if (saved) restore(saved, true); return; }
    if (button.dataset.view) { openView(button.dataset.view); return; }
    if (button.dataset.enter) { openView(button.dataset.enter); return; }
    if (button.dataset.inspect) { navigateNode(button.dataset.inspect); return; }
    if (button.dataset.select) { selectNode(button.dataset.select); return; }
    if (button.dataset.node) {
      if (state.mode !== 'map') { navigateNode(button.dataset.node); return; }
      selectNode(button.dataset.node);
      return;
    }
    if (button.dataset.relation) {
      state.relation = button.dataset.relation; render();
      workspace.querySelector(`[data-relation="${state.relation}"]`).focus({ preventScroll: true });
      return;
    }
    if (button.dataset.scenario !== undefined) { state.scenario = Number(button.dataset.scenario); state.choice = 0; state.step = 0; render(); workspace.querySelector(`[data-scenario="${state.scenario}"]`).focus({ preventScroll: true }); return; }
    if (button.dataset.topic !== undefined) { state.topic = Number(button.dataset.topic); render(); workspace.querySelector(`[data-topic="${state.topic}"]`).focus({ preventScroll: true }); return; }
    if (!['request', 'scenarios'].includes(state.mode)) return;
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
    event.preventDefault(); openView('overview');
  });
  document.getElementById('intro').innerHTML = rich(data.intro);
  document.title = `${plain(data.title)} · rustX`;
  document.getElementById('baseline').innerHTML = `<span class="baseline-tag">{{source-code}} 基线</span><details class="baseline-details"><summary><code>${escape(data.baseline.commit.slice(0, 8))}</code> <span data-verbatim>${escape(data.baseline.branch)}</span><span aria-hidden="true">⌄</span></summary><div><code>${escape(data.baseline.commit)}</code><p lang="en" data-verbatim>${escape(data.baseline.subject)}</p><p>本导览基于固定的 {{snapshot-source}}，不会跟踪当前工作区，也不会执行请求。</p></div></details><span class="simulation-label">教学模拟</span>`;
  render();
  new ResizeObserver(() => {
    drawLines();
    revealCurrentStep();
  }).observe(workspace);
})();
