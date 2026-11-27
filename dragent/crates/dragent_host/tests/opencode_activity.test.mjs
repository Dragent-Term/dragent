import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const source = (await readFile(new URL('../src/opencode_activity.mjs', import.meta.url), 'utf8'))
  .replace('__DRAGENT_HELPER_ARGS__', '[]');
const { DragentSession } = await import(`data:text/javascript,${encodeURIComponent(source)}`);
// Reports are queued, not awaited by OpenCode: let the queue drain.
const settle = () => new Promise(resolve => setTimeout(resolve, 0));
async function setup() {
  const reports = [];
  globalThis.Bun = { spawn: ({ stdin }) => {
    reports.push(JSON.parse(new TextDecoder().decode(stdin)));
    return { exited: Promise.resolve(0) };
  }};
  const plugin = await DragentSession({ client: { session: { get: async ({ path }) => ({ data: {
    id: path.id, parentID: path.id === 'child' ? 'a' : undefined,
  }}) }}});
  const event = (type, sessionID = 'a', extra = {}) => plugin.event({ event: {
    type, properties: { sessionID, ...extra },
  }});
  return { plugin, reports, event };
}
test('prompt, busy, idle: exactly one start and one completion', async () => {
  const { plugin, reports, event } = await setup();
  await plugin['chat.message']({ sessionID: 'a' });
  await event('session.status', 'a', { status: { type: 'busy' } });
  await event('session.status', 'a', { status: { type: 'idle' } });
  await event('session.idle');
  await settle();
  assert.deepEqual(reports.map(r => r.hook_event_name), ['UserPromptSubmit', 'Stop']);
});
test('permission and questions resume, then stop; child sessions cannot stop parent', async () => {
  const { reports, event } = await setup();
  await event('session.busy');
  await event('session.idle', 'child');
  await event('session.idle', 'other-root');
  await event('question.asked');
  await event('question.replied');
  await event('session.idle');
  await settle();
  assert.deepEqual(reports.map(r => r.hook_event_name), ['Start', 'PermissionRequest', 'Start', 'Stop']);
});
test('failure is not overwritten by the following idle event; next prompt starts a new turn', async () => {
  const { plugin, reports, event } = await setup();
  await event('session.busy');
  await event('session.error');
  await event('session.idle');
  await plugin['chat.message']({ sessionID: 'a' });
  await event('session.idle');
  await settle();
  assert.deepEqual(reports.map(r => r.hook_event_name), ['Start', 'Failed', 'UserPromptSubmit', 'Stop']);
});
test('concurrent callbacks remain ordered', async () => {
  const { event, reports } = await setup();
  await Promise.all([event('session.busy'), event('session.idle')]);
  await settle();
  assert.deepEqual(reports.map(r => r.hook_event_name), ['Start', 'Stop']);
});

test('two instances of the same provider keep separate session ownership', async () => {
  const { event, reports } = await setup();
  const second = await DragentSession({ client: { session: { get: async ({ path }) => ({ data: { id: path.id } }) }}});
  await event('session.busy', 'a');
  await second.event({ event: { type: 'session.busy', properties: { sessionID: 'b' } }});
  await event('session.idle', 'b');
  await second.event({ event: { type: 'session.idle', properties: { sessionID: 'b' } }});
  await event('session.idle', 'a');
  await settle();
  assert.deepEqual(reports.map(r => [r.session_id, r.hook_event_name]), [
    ['a', 'Start'], ['b', 'Start'], ['b', 'Stop'], ['a', 'Stop'],
  ]);
});

test('a slow helper never holds up OpenCode', async () => {
  const { plugin, reports } = await setup();
  let finish;
  globalThis.Bun = { spawn: ({ stdin }) => {
    reports.push(JSON.parse(new TextDecoder().decode(stdin)));
    return { exited: new Promise(resolve => { finish = resolve; }) };
  }};
  assert.equal(plugin['chat.message']({ sessionID: 'a' }), undefined);
  assert.equal(plugin['permission.ask']({ sessionID: 'a' }, { status: 'ask' }), undefined);
  await settle();
  assert.deepEqual(reports.map(r => r.hook_event_name), ['UserPromptSubmit']);
  finish(0);
  await settle();
  assert.deepEqual(reports.map(r => r.hook_event_name), ['UserPromptSubmit', 'PermissionRequest']);
});
