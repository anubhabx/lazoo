import { buildViewModel } from './view-model.js';

const elements = {
  activityNote: document.querySelector('#activity-note'),
  banner: document.querySelector('#connection-banner'),
  bannerCode: document.querySelector('#banner-code'),
  bannerMessage: document.querySelector('#banner-message'),
  bannerRetry: document.querySelector('#banner-retry'),
  connectionLabel: document.querySelector('#connection-label'),
  eventList: document.querySelector('#event-list'),
  footerRefresh: document.querySelector('#footer-refresh'),
  healthDetail: document.querySelector('#health-detail'),
  healthLabel: document.querySelector('#health-label'),
  liveRegion: document.querySelector('#live-region'),
  metricEvents: document.querySelector('#metric-events'),
  metricIncidents: document.querySelector('#metric-incidents'),
  metricOutbox: document.querySelector('#metric-outbox'),
  metricProjects: document.querySelector('#metric-projects'),
  projectList: document.querySelector('#project-list'),
  projectsNote: document.querySelector('#projects-note'),
  refreshButton: document.querySelector('#refresh-button'),
  systemBoot: document.querySelector('#system-boot'),
  systemContract: document.querySelector('#system-contract'),
  systemDataRoot: document.querySelector('#system-data-root'),
  systemJournal: document.querySelector('#system-journal'),
  systemKeeper: document.querySelector('#system-keeper'),
  systemSnapshot: document.querySelector('#system-snapshot'),
  systemUptime: document.querySelector('#system-uptime'),
  systemVersion: document.querySelector('#system-version'),
};

let refreshInFlight = false;
let lastSnapshot = null;

function setText(target, value) {
  target.textContent = String(value ?? '—');
}

function createElement(tagName, className, text) {
  const node = document.createElement(tagName);
  if (className) node.className = className;
  if (text !== undefined) setText(node, text);
  return node;
}

function emptyState(message, command) {
  const wrapper = createElement('div', 'empty-state');
  wrapper.append(createElement('p', '', message));
  if (command) wrapper.append(createElement('code', '', command));
  return wrapper;
}

function renderProjects(projects) {
  elements.projectList.replaceChildren();
  setText(elements.projectsNote, `${projects.length} ${projects.length === 1 ? 'project' : 'projects'}`);
  if (projects.length === 0) {
    elements.projectList.append(
      emptyState(
        'No project has joined this installation yet. Register a Git repository from the companion CLI.',
        'lazooctl project register C:\\path\\to\\repository',
      ),
    );
    return;
  }

  for (const project of projects) {
    const row = createElement('article', 'project-row');
    row.append(createElement('h3', 'project-name', project.name));
    row.append(createElement('p', 'project-path', project.path));
    const meta = createElement('p', 'project-meta');
    meta.append(document.createTextNode(project.checkoutLabel));
    meta.append(document.createElement('br'));
    meta.append(document.createTextNode(`Revision ${project.revision}`));
    row.append(meta);
    elements.projectList.append(row);
  }
}

function renderEvents(events) {
  elements.eventList.replaceChildren();
  setText(elements.activityNote, events.length > 0 ? 'Newest event first' : 'Ledger is quiet');
  if (events.length === 0) {
    elements.eventList.append(
      emptyState('No durable activity has been recorded. Project registration will create the first event.'),
    );
    return;
  }

  for (const event of events) {
    const row = createElement('li', 'event-row');
    row.title = event.timestamp;
    row.append(createElement('span', 'event-sequence', `#${event.sequence}`));
    row.append(createElement('p', 'event-label', event.label));
    row.append(createElement('p', 'event-summary', event.summary));
    row.append(createElement('time', 'event-time', event.occurredAt));
    elements.eventList.append(row);
  }
}

function renderSystem(model) {
  setText(elements.systemSnapshot, model.capturedAt);
  setText(elements.systemVersion, `v${model.system.daemonVersion}`);
  setText(elements.systemUptime, model.system.uptime);
  setText(
    elements.systemContract,
    `Protocol ${model.system.protocolVersion} / schema ${model.system.schemaVersion}`,
  );
  setText(elements.systemJournal, model.system.journalMode);
  setText(elements.systemKeeper, model.system.keeper);
  setText(elements.systemBoot, model.system.bootId);
  setText(elements.systemDataRoot, model.system.dataRoot);
}

function renderSnapshot(snapshot) {
  const model = buildViewModel(snapshot);
  lastSnapshot = snapshot;
  const degraded = model.health.tone === 'warning';
  document.body.dataset.connection = degraded ? 'degraded' : 'connected';
  elements.banner.hidden = true;
  setText(elements.connectionLabel, degraded ? 'Needs attention' : 'Connected');
  setText(elements.healthLabel, model.health.label);
  setText(elements.healthDetail, model.health.detail);
  setText(elements.metricProjects, model.metrics.projects);
  setText(elements.metricEvents, model.metrics.events);
  setText(elements.metricOutbox, model.metrics.pendingOutbox);
  setText(elements.metricIncidents, model.metrics.incidents);
  setText(elements.footerRefresh, `Updated ${model.lastUpdated}`);
  renderProjects(model.projects);
  renderEvents(model.events);
  renderSystem(model);
  setText(elements.liveRegion, `${model.health.label}. Snapshot updated.`);
}

function normalizeFailure(error) {
  if (error && typeof error === 'object') {
    return {
      code: String(error.code || 'LZ-DESKTOP-UNKNOWN'),
      message: String(error.message || 'The desktop could not load a control-plane snapshot.'),
    };
  }
  return {
    code: 'LZ-DESKTOP-UNKNOWN',
    message: typeof error === 'string' ? error : 'The desktop could not load a control-plane snapshot.',
  };
}

function renderFailure(error) {
  const failure = normalizeFailure(error);
  document.body.dataset.connection = 'offline';
  elements.banner.hidden = false;
  setText(elements.bannerCode, failure.code);
  setText(elements.bannerMessage, failure.message);
  setText(elements.connectionLabel, 'Disconnected');
  setText(elements.footerRefresh, 'Connection lost');
  if (!lastSnapshot) {
    setText(elements.healthLabel, 'Control room disconnected');
    setText(elements.healthDetail, failure.message);
    setText(elements.projectsNote, 'Daemon unavailable');
    setText(elements.activityNote, 'Daemon unavailable');
    elements.projectList.replaceChildren(
      emptyState(
        'Start lazoo-keeper, then retry the local connection.',
        '.\\target\\release\\lazoo-keeper.exe',
      ),
    );
    elements.eventList.replaceChildren(
      emptyState('The last durable events will appear after the daemon reconnects.'),
    );
  }
  setText(elements.liveRegion, `${failure.code}. ${failure.message}`);
}

async function refreshSnapshot() {
  if (refreshInFlight) return;
  refreshInFlight = true;
  elements.refreshButton.disabled = true;
  elements.refreshButton.setAttribute('aria-busy', 'true');
  try {
    const invoke = window.__TAURI__?.core?.invoke;
    if (typeof invoke !== 'function') {
      throw {
        code: 'LZ-DESKTOP-BRIDGE-MISSING',
        message: 'The secure Tauri bridge is unavailable in this window.',
      };
    }
    const snapshot = await invoke('load_desktop_snapshot');
    renderSnapshot(snapshot);
  } catch (error) {
    renderFailure(error);
  } finally {
    refreshInFlight = false;
    elements.refreshButton.disabled = false;
    elements.refreshButton.setAttribute('aria-busy', 'false');
  }
}

elements.refreshButton.addEventListener('click', refreshSnapshot);
elements.bannerRetry.addEventListener('click', refreshSnapshot);
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible') refreshSnapshot();
});

refreshSnapshot();
window.setInterval(() => {
  if (document.visibilityState === 'visible') refreshSnapshot();
}, 10_000);
