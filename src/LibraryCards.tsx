import { useMemo, useRef, type DragEvent, type KeyboardEvent, type ReactNode } from 'react';
import { GripVertical, Star } from 'lucide-react';
import { cardNavigationIndex, displayStatus, statusExplanation, type LibraryItem } from './library';
import { categoryColors, type Category } from './categories';
import { CategoryChip } from './CategoryColor';
import './library-cards.css';

export type LibraryCardItem = LibraryItem & { version?: string | null };
export type LibraryCardsProps<T extends LibraryCardItem> = {
  items: readonly T[]; selectedIds: readonly string[]; categories: readonly Category[];
  onChoose: (id: string, checked: boolean, shift: boolean) => void;
  onSelectAll: () => void; onOpenSkill: (skill: T) => void; onToggleFavorite: (skill: T) => void;
  onDragStart: (event: DragEvent, skill: T) => void; onDragEnd?: () => void;
  renderActions: (skill: T) => ReactNode; renderSource?: (skill: T) => ReactNode;
  connectionCount?: (skill: T) => number; disabled?: boolean; showCategory?: boolean;
};

/** Uses the parent's exact query/page/selection; cards never create another inventory. */
export function LibraryCards<T extends LibraryCardItem>({ items, selectedIds, categories, onChoose, onSelectAll, onOpenSkill, onToggleFavorite, onDragStart, onDragEnd, renderActions, renderSource, connectionCount, disabled = false, showCategory = true }: LibraryCardsProps<T>) {
  const root = useRef<HTMLDivElement>(null), pointerShift = useRef(false);
  const selected = useMemo(() => new Set(selectedIds), [selectedIds]);
  const colors = useMemo(() => categoryColors(categories), [categories]);
  const keys = (event: KeyboardEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    if (target.isContentEditable || target.closest('textarea,select,input:not([type=checkbox])')) return;
    if ((event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey && event.key.toLowerCase() === 'a') {
      event.preventDefault(); event.stopPropagation(); if (!disabled) onSelectAll(); return;
    }
    const card = target.closest<HTMLElement>('[data-skill-id]');
    if (!card || target !== card || event.ctrlKey || event.metaKey || event.altKey) return;
    const index = items.findIndex(item => item.id === card.dataset.skillId);
    if (event.key === ' ') { event.preventDefault(); event.stopPropagation(); if (!disabled && index >= 0) onChoose(items[index].id, !selected.has(items[index].id), event.shiftKey); return; }
    const nodes = Array.from(root.current?.querySelectorAll<HTMLElement>(':scope > [data-skill-id]') || []);
    const firstTop = nodes[0]?.getBoundingClientRect().top;
    const columns = nodes.filter(node => Math.abs(node.getBoundingClientRect().top - firstTop!) < 2).length;
    const next = cardNavigationIndex(index, items.length, columns, event.key);
    if (next !== null) { event.preventDefault(); event.stopPropagation(); if (event.shiftKey && !disabled) onChoose(items[next].id, true, true); nodes[next]?.focus({ preventScroll: true }); nodes[next]?.scrollIntoView({ block: 'nearest', inline: 'nearest' }); }
  };
  return <div className="library-card-grid" ref={root} role="group" aria-label="Skill 卡片列表" onKeyDown={keys}>{items.map(skill => {
    const chosen = selected.has(skill.id), name = skill.displayName || skill.name, status = displayStatus(skill);
    const category = categories.find(item => item.id === skill.categoryId), count = connectionCount?.(skill);
    return <article key={skill.id} className={`library-card ${chosen ? 'is-selected' : ''}`} data-skill-id={skill.id} data-category-color={category ? colors.get(category.id) : undefined} tabIndex={0} draggable={!disabled && chosen} onDragStart={event => { if ((event.target as Element).closest('button,input,a,select,textarea')) { event.preventDefault(); return; } onDragStart(event, skill); }} onDragEnd={onDragEnd} aria-label={`${name}，${status.label}${chosen ? '，已选择' : ''}`}>
      <div className="library-card-top"><input type="checkbox" aria-label={`选择 ${name}`} checked={chosen} disabled={disabled} onClick={event => { pointerShift.current = event.shiftKey; }} onChange={event => { onChoose(skill.id, event.target.checked, pointerShift.current); pointerShift.current = false; }} /><button type="button" className="skill-drag-handle icon-button" aria-label={`拖动 ${name} 到分类`} title="拖入左侧分类；已选时一起移动所选 Skill" draggable={!disabled} disabled={disabled} onDragStart={event => onDragStart(event, skill)} onDragEnd={onDragEnd}><GripVertical size={16} /></button><span className={`badge ${status.tone}`} title={statusExplanation(skill)}>{status.label}</span><button type="button" className={`library-card-favorite icon-button ${skill.favorite ? 'active' : ''}`} aria-label={`${skill.favorite ? '取消收藏' : '收藏'} ${name}`} aria-pressed={!!skill.favorite} title={skill.favorite ? '取消收藏' : '收藏'} disabled={disabled} onClick={() => onToggleFavorite(skill)}><Star size={17} fill={skill.favorite ? 'currentColor' : 'none'} /></button></div>
      <button type="button" className="library-card-name" onClick={() => onOpenSkill(skill)} title={`查看 ${name}${name !== skill.name ? ` · ${skill.name}` : ''}`}><strong>{name}</strong></button>
      {skill.description && <p className="library-card-description" title={skill.description}>{skill.description}</p>}
      {(showCategory || skill.version || !!count) && <div className="library-card-category">{showCategory && <CategoryChip category={category} color={category ? colors.get(category.id) : undefined} />}{skill.version && <span className="library-card-version">{skill.version}</span>}{!!count && <span className="library-card-connections" title={`${count} 个客户端连接`}>{count} 个连接</span>}</div>}
      <div className="library-card-source">{renderSource ? renderSource(skill) : <span title={skill.localPath || skill.repo || undefined}>{skill.repo || (skill.localPath ? '本地 Skill' : '')}</span>}{skill.readOnly && <span className="library-card-readonly">只读</span>}</div>
      <div className="library-card-actions">{renderActions(skill)}</div>
    </article>;
  })}</div>;
}
