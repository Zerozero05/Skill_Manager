export type ExportResourceGroup = 'required' | 'optional_package' | 'optional_external' | 'environment' | 'generated';
export type ExportResourceSource = 'local' | 'github' | 'generated' | 'none';
export type ExportResource = {
  id: string;
  label: string;
  path: string;
  group: ExportResourceGroup;
  included: boolean;
  selectable: boolean;
  materializable?: boolean;
  source: ExportResourceSource;
  size?: number;
  zipPath?: string;
  reason: string;
  referenceIds: string[];
  localPath?: string;
};
export type ExportManifestEntry = { path: string; resourceId?: string; source?: ExportResourceSource; size?: number };
export type ExportPlan = {
  id: string;
  skillId: string;
  remoteCommit?: string;
  remoteStatus?: string | null;
  remoteError?: string | null;
  resources: ExportResource[];
  manifest: ExportManifestEntry[];
  summary: {
    required: number;
    optionalPackageSelected: number;
    optionalPackageTotal: number;
    optionalExternalSelected: number;
    optionalExternalTotal: number;
    generated: number;
    total: number;
    remoteCount: number;
    bytes: number;
  };
  canExport: boolean;
  issues: string[];
};

export type ExportPlanRead = { key: string; contextKey: string; plan: ExportPlan | null; error: string; complete: boolean; fromCache: boolean };

export function exportPlanReadKeys(contextKey: string, selection: string, refreshRevision: number) {
  const cacheKey = JSON.stringify([contextKey, selection]);
  return { cacheKey, requestKey: JSON.stringify([cacheKey, refreshRevision]) };
}

/** A selection refresh keeps its resource rows mounted, without authorizing export. */
export function exportPlanView(result: ExportPlanRead, key: string, contextKey: string, cached?: ExportPlan, retain = true) {
  const current = result.key === key && result.contextKey === contextKey;
  const plan = (current ? result.plan : null) ?? cached ?? (retain && result.contextKey === contextKey ? result.plan : null);
  return { plan, current: current && result.complete && !result.error };
}

export const EXPORT_GROUPS: { id: ExportResourceGroup; label: string; description: string }[] = [
  { id: 'required', label: '必要文件', description: '自动包含，无需重复勾选。' },
  { id: 'optional_package', label: '可选包内文件', description: '仅本次勾选后加入 ZIP，不改变运行清单。' },
  { id: 'optional_external', label: '可选外部文件', description: '包括可携带的共享规则；仅本次加入，不改变长期用途。' },
  { id: 'environment', label: '环境与参考', description: '此处不加入 ZIP，逐项显示原因。' },
];

/** IDs come from the same backend resource inventory used by the ZIP writer. */
export function optionalExportIds(resources: readonly ExportResource[]): string[] {
  return [...new Set(resources.filter(resource => resource.selectable && resource.materializable !== false && ['local', 'github'].includes(resource.source) && ['optional_package', 'optional_external'].includes(resource.group)).map(resource => resource.id))];
}

export function filterExportResources(resources: readonly ExportResource[], query: string, includedOnly = false): ExportResource[] {
  const words = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  return resources.filter(resource => (!includedOnly || resource.included) && words.every(word => `${resource.label} ${resource.path} ${resource.zipPath || ''} ${resource.reason}`.toLocaleLowerCase().includes(word)));
}

export function resourceForReference(resources: readonly ExportResource[], referenceId: string): ExportResource | undefined {
  return resources.find(resource => resource.referenceIds.includes(referenceId));
}

/** Only an authoritative shared resource ID permits collapsing standalone references. */
export function clusterExportReferences<T extends { id: string }>(references: readonly T[], resources: readonly ExportResource[]): T[][] {
  const groups = new Map<string, T[]>();
  for (const reference of references) {
    const resource = resourceForReference(resources, reference.id);
    const key = resource ? `resource:${resource.id}` : `reference:${reference.id}`;
    groups.set(key, [...(groups.get(key) || []), reference]);
  }
  return [...groups.values()];
}

export function exportInclusionLabel(resource: ExportResource): string {
  if (resource.source === 'generated') return resource.included && resource.materializable !== false ? '自动生成并包含' : '生成记录待处理';
  if (resource.group === 'required') return resource.included && resource.source !== 'none' && resource.materializable !== false ? '自动包含' : '必要文件待处理';
  if (resource.included && (resource.source === 'none' || resource.materializable === false)) return '已选文件待处理，可取消选择';
  if (resource.group === 'environment' || !resource.selectable && !resource.included) return '不加入 ZIP';
  return resource.included ? '本次包含' : '未选择';
}

/** A selected failed optional resource stays removable; a missing plan never implies success. */
export function exportResourceState(resource?: ExportResource, evidencePending = false) {
  if (!resource || evidencePending && (resource.group === 'required' || resource.source === 'generated')) return { automatic: false, available: false, selectable: false, problem: false, label: '导出范围待核对' };
  const automatic = resource.group === 'required' || resource.source === 'generated';
  const available = resource.source !== 'none' && resource.materializable !== false && (automatic ? resource.included : resource.selectable);
  return {
    automatic,
    available,
    selectable: !automatic && (resource.included || resource.group !== 'environment' && available),
    problem: automatic && !available || resource.included && !available,
    label: exportInclusionLabel(resource),
  };
}

export function resourcePathParts(path: string) {
  const value = path.replaceAll('\\', '/').replace(/\/+$/, '');
  const split = value.lastIndexOf('/');
  return { name: value.slice(split + 1) || path, directory: split >= 0 ? value.slice(0, split) : '' };
}

export function exportSourceLabel(source: ExportResourceSource): string {
  return ({ local: '本地', github: 'GitHub', generated: '程序生成', none: '不可用' } as const)[source];
}
