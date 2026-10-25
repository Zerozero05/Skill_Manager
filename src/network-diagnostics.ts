export type NetworkDiagnostic = { ok: boolean; status?: number; durationMs: number; error?: string };

/** HTTP failures have status codes without a transport error; ok is authoritative. */
export function networkDiagnosticView(result: NetworkDiagnostic) {
  const success = result.ok === true && !result.error;
  const status = result.status ? `HTTP ${result.status}` : '';
  return { success, text: result.error || [success ? '连接正常' : '检查未通过', status].filter(Boolean).join(' · ') };
}
