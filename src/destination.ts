export type DestinationMode = 'default' | 'custom';
export type DestinationDraft = { mode: DestinationMode | null; path: string };

export function destinationDraft(value?: unknown): DestinationDraft {
  const raw = value && typeof value === 'object' ? value as Record<string, unknown> : {};
  return { mode: raw.mode === 'default' || raw.mode === 'custom' ? raw.mode : null, path: typeof raw.path === 'string' ? raw.path : '' };
}

/** A changed default must be chosen again; an explicit custom choice stays in this operation. */
export function destinationReady(choice: DestinationDraft, defaultPath: string): boolean {
  return !!choice.path.trim() && (choice.mode === 'custom' || choice.mode === 'default' && !!defaultPath && choice.path === defaultPath);
}

export function destinationAfterPicker(previous: DestinationDraft, path: string | null | undefined): DestinationDraft {
  return path ? { mode: 'custom', path } : previous;
}
