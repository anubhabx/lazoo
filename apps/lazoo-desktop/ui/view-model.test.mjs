import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildViewModel,
  formatCheckoutMode,
  formatRelativeTime,
  formatTimestamp,
  formatUptime,
  humanizeEventType,
  summarizeEvent,
} from './view-model.js';

const NOW = Date.parse('2026-08-28T12:00:00Z');

function snapshot(statusOverrides = {}) {
  return {
    capturedAt: '2026-08-28T11:59:55Z',
    status: {
      daemonVersion: '0.1.0',
      bootId: '00000000-0000-0000-0000-000000000001',
      uptimeMillis: 3_660_000,
      protocolVersion: 1,
      dataRoot: 'C:\\Users\\owner\\AppData\\Local\\Lazoo',
      schemaVersion: 1,
      journalMode: 'wal',
      projectCount: 1,
      eventHighWater: 7,
      pendingOutbox: 0,
      emergencyIncidentCount: 0,
      keeper: null,
      degraded: [],
      ...statusOverrides,
    },
    projects: [
      {
        id: '00000000-0000-0000-0000-000000000002',
        repositoryIdentity: 'example',
        canonicalPath: 'C:\\workspace\\Example',
        displayName: 'Example',
        checkoutMode: 'isolated',
        configurationRevision: 1,
        createdAt: '2026-08-28T11:58:00Z',
      },
    ],
    events: [
      {
        sequence: 7,
        id: '00000000-0000-0000-0000-000000000003',
        eventType: 'project.registered',
        payload: { displayName: 'Example' },
        createdAt: '2026-08-28T11:59:00Z',
      },
    ],
  };
}

test('formats daemon uptime without sub-minute noise', () => {
  assert.equal(formatUptime(0), '< 1 min');
  assert.equal(formatUptime(59_999), '< 1 min');
  assert.equal(formatUptime(3_660_000), '1 hr 1 min');
  assert.equal(formatUptime(183_600_000), '2 days 3 hrs');
});

test('formats timestamps and relative time defensively', () => {
  assert.equal(formatRelativeTime('2026-08-28T11:59:30Z', NOW), '30 sec ago');
  assert.equal(formatRelativeTime('2026-08-28T11:55:00Z', NOW), '5 min ago');
  assert.equal(formatRelativeTime('not-a-date', NOW), 'Time unavailable');
  assert.equal(formatTimestamp('not-a-date'), 'Time unavailable');
  assert.match(formatTimestamp('2026-08-28T11:59:30Z'), /2026/);
});

test('labels checkout modes and event types for humans', () => {
  assert.equal(formatCheckoutMode('isolated'), 'Isolated worktree');
  assert.equal(formatCheckoutMode('lead-main'), 'Lead uses main');
  assert.equal(formatCheckoutMode('future-mode'), 'Future mode');
  assert.equal(humanizeEventType('project.registered'), 'Project registered');
  assert.equal(humanizeEventType('LZ_DAEMON_RECOVERED'), 'LZ daemon recovered');
});

test('summarizes known and unknown events without dumping raw JSON', () => {
  assert.equal(
    summarizeEvent({
      eventType: 'project.registered',
      payload: { displayName: 'Orchestrator' },
    }),
    'Orchestrator joined the zoo',
  );
  assert.equal(
    summarizeEvent({
      eventType: 'incident.recovered',
      payload: { code: 'LZ-DAEMON-UNAVAILABLE' },
    }),
    'Recovered LZ-DAEMON-UNAVAILABLE',
  );
  const longValue = 'x'.repeat(200);
  const summary = summarizeEvent({ eventType: 'custom.event', payload: { detail: longValue } });
  assert.ok(summary.length <= 123);
  assert.ok(summary.endsWith('…'));
});

test('builds healthy operational view model', () => {
  const model = buildViewModel(snapshot(), NOW);

  assert.deepEqual(model.health, {
    tone: 'healthy',
    label: 'Control plane steady',
    detail: 'Daemon and event ledger are responding.',
  });
  assert.equal(model.lastUpdated, '5 sec ago');
  assert.equal(model.projects[0].checkoutLabel, 'Isolated worktree');
  assert.equal(model.events[0].summary, 'Example joined the zoo');
  assert.equal(model.system.uptime, '1 hr 1 min');
  assert.equal(model.system.journalMode, 'WAL');
});

test('builds degraded health when daemon reports reasons', () => {
  const model = buildViewModel(snapshot({ degraded: ['outbox delayed'] }), NOW);

  assert.equal(model.health.label, 'Attention needed');
  assert.equal(model.health.tone, 'warning');
  assert.equal(model.health.detail, 'outbox delayed');
});

test('prioritizes recovered incident evidence in health state', () => {
  const model = buildViewModel(snapshot({ emergencyIncidentCount: 2 }), NOW);

  assert.equal(model.health.label, 'Recovery evidence found');
  assert.equal(model.health.tone, 'warning');
  assert.match(model.health.detail, /2 recovered incidents/);
});
