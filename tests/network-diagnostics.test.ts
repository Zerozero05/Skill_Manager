import assert from 'node:assert/strict';
import test from 'node:test';
import { networkDiagnosticView } from '../src/network-diagnostics.ts';

test('HTTP failure statuses never render as a successful connection without a transport error', () => {
  for (const status of [403, 404, 500]) {
    const view = networkDiagnosticView({ ok: false, status, durationMs: 10 });
    assert.equal(view.success, false); assert.match(view.text, /检查未通过/); assert(view.text.includes(`HTTP ${status}`));
  }
});

test('successful HTTP diagnostics and transport failures remain distinct', () => {
  assert.deepEqual(networkDiagnosticView({ ok: true, status: 200, durationMs: 12 }), { success: true, text: '连接正常 · HTTP 200' });
  assert.deepEqual(networkDiagnosticView({ ok: false, durationMs: 13, error: '连接超时' }), { success: false, text: '连接超时' });
});
