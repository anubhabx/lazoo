const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const SUMMARY_LIMIT = 122;

function plural(value, singular, pluralForm = `${singular}s`) {
  return `${value} ${value === 1 ? singular : pluralForm}`;
}

function titleCase(value) {
  return String(value ?? '')
    .trim()
    .split(/[._\-\s]+/u)
    .filter(Boolean)
    .map((part, index) => {
      if (/^[A-Z0-9]{2,3}$/u.test(part)) return part;
      const lowered = part.toLocaleLowerCase();
      return index === 0 ? `${lowered.charAt(0).toLocaleUpperCase()}${lowered.slice(1)}` : lowered;
    })
    .join(' ');
}

function truncate(value, limit = SUMMARY_LIMIT) {
  const normalized = String(value ?? '').replace(/\s+/gu, ' ').trim();
  if (normalized.length <= limit) return normalized;
  return `${normalized.slice(0, limit - 1).trimEnd()}…`;
}

export function formatUptime(milliseconds) {
  const value = Number(milliseconds);
  if (!Number.isFinite(value) || value < MINUTE) return '< 1 min';

  if (value >= DAY) {
    const days = Math.floor(value / DAY);
    const hours = Math.floor((value % DAY) / HOUR);
    return hours > 0 ? `${plural(days, 'day')} ${plural(hours, 'hr')}` : plural(days, 'day');
  }

  if (value >= HOUR) {
    const hours = Math.floor(value / HOUR);
    const minutes = Math.floor((value % HOUR) / MINUTE);
    return minutes > 0
      ? `${plural(hours, 'hr')} ${plural(minutes, 'min')}`
      : plural(hours, 'hr');
  }

  return plural(Math.floor(value / MINUTE), 'min');
}

export function formatTimestamp(value) {
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) return 'Time unavailable';
  return new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  }).format(new Date(parsed));
}

export function formatRelativeTime(value, now = Date.now()) {
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) return 'Time unavailable';
  const elapsed = Math.max(0, Number(now) - parsed);
  if (elapsed < MINUTE) return `${Math.floor(elapsed / 1000)} sec ago`;
  if (elapsed < HOUR) return `${Math.floor(elapsed / MINUTE)} min ago`;
  if (elapsed < DAY) return `${Math.floor(elapsed / HOUR)} hr ago`;
  return `${Math.floor(elapsed / DAY)} day ago`;
}

export function formatCheckoutMode(mode) {
  if (mode === 'isolated') return 'Isolated worktree';
  if (mode === 'lead-main') return 'Lead uses main';
  return titleCase(mode) || 'Unknown checkout';
}

export function humanizeEventType(eventType) {
  return titleCase(eventType) || 'Control-plane event';
}

export function summarizeEvent(event) {
  const type = event?.eventType;
  const payload = event?.payload && typeof event.payload === 'object' ? event.payload : {};

  if (type === 'project.registered') {
    return `${truncate(payload.displayName || payload.projectName || 'A project')} joined the zoo`;
  }
  if (type === 'incident.recovered' || type === 'emergency.incident.imported') {
    return `Recovered ${truncate(payload.code || payload.incidentCode || 'an incident')}`;
  }

  const candidate =
    payload.summary ??
    payload.message ??
    payload.detail ??
    payload.displayName ??
    payload.code ??
    payload.canonicalPath;
  return truncate(candidate || 'Recorded by the control plane');
}

export function buildViewModel(snapshot, now = Date.now()) {
  const status = snapshot?.status ?? {};
  const degraded = Array.isArray(status.degraded) ? status.degraded.filter(Boolean) : [];
  const incidentCount = Number(status.emergencyIncidentCount) || 0;
  let health = {
    tone: 'healthy',
    label: 'Control plane steady',
    detail: 'Daemon and event ledger are responding.',
  };
  if (degraded.length > 0) {
    health = {
      tone: 'warning',
      label: 'Attention needed',
      detail: degraded.join(' · '),
    };
  } else if (incidentCount > 0) {
    health = {
      tone: 'warning',
      label: 'Recovery evidence found',
      detail: `${incidentCount} recovered ${incidentCount === 1 ? 'incident' : 'incidents'} in the ledger.`,
    };
  }

  const projects = (Array.isArray(snapshot?.projects) ? snapshot.projects : []).map((project) => ({
    id: project.id,
    name: project.displayName || 'Unnamed project',
    path: project.canonicalPath || 'Path unavailable',
    checkoutLabel: formatCheckoutMode(project.checkoutMode),
    revision: Number(project.configurationRevision) || 0,
    createdAt: formatTimestamp(project.createdAt),
  }));

  const events = (Array.isArray(snapshot?.events) ? [...snapshot.events] : [])
    .sort((left, right) => Number(right.sequence) - Number(left.sequence))
    .map((event) => ({
      id: event.id,
      sequence: Number(event.sequence) || 0,
      label: humanizeEventType(event.eventType),
      summary: summarizeEvent(event),
      occurredAt: formatRelativeTime(event.createdAt, now),
      timestamp: formatTimestamp(event.createdAt),
    }));

  return {
    health,
    lastUpdated: formatRelativeTime(snapshot?.capturedAt, now),
    capturedAt: formatTimestamp(snapshot?.capturedAt),
    metrics: {
      projects: Number(status.projectCount) || projects.length,
      events: Number(status.eventHighWater) || 0,
      pendingOutbox: Number(status.pendingOutbox) || 0,
      incidents: incidentCount,
    },
    projects,
    events,
    system: {
      daemonVersion: status.daemonVersion || 'Unknown',
      protocolVersion: Number(status.protocolVersion) || 0,
      schemaVersion: Number(status.schemaVersion) || 0,
      journalMode: String(status.journalMode || 'unknown').toLocaleUpperCase(),
      bootId: status.bootId || 'Unavailable',
      uptime: formatUptime(status.uptimeMillis),
      dataRoot: status.dataRoot || 'Unavailable',
      keeper: status.keeper ? 'Supervising' : 'State unavailable',
    },
  };
}
