import { createContext, useContext } from 'react';
import type { useTasks } from './tasks';
import type { WorkspacePane } from './workspace';

export type SessionDraft = Record<string, unknown>;
export type SessionBridge = { capture: () => SessionDraft; flush: () => Promise<boolean> };
export type PaneFocusNavigation = { view?: string; clientId?: string; section?: string };
export type WorkspaceSession = {
  pane: WorkspacePane;
  active: boolean;
  visible: boolean;
  owned: boolean;
  isTransferring: () => boolean;
  closing: boolean;
  initialDraft: SessionDraft;
  managerSnapshot: unknown;
  tasks: ReturnType<typeof useTasks>;
  acceptSnapshot: (value: unknown) => void;
  register: (id: string, bridge: SessionBridge) => () => void;
  openDetail: (skill: { id: string; name: string; displayName?: string }, section: 'docs' | 'files' | 'source') => void;
  openHistory: () => void;
  openOperation: (modal: unknown, draft: SessionDraft, title: string, skillId?: string) => void;
  updateNavigation: (id: string, patch: Record<string, unknown>) => void;
  goHome: (view?: string, clientId?: string) => void;
  closePane: (id: string) => void;
  focusPane: (id: string, navigation?: PaneFocusNavigation) => void;
  requestStorageRestart: (previewId: string) => Promise<void>;
};

export const WorkspaceSessionContext = createContext<WorkspaceSession | null>(null);
export function useWorkspaceSession(): WorkspaceSession {
  const value = useContext(WorkspaceSessionContext);
  if (!value) throw new Error('Workspace session is unavailable');
  return value;
}
