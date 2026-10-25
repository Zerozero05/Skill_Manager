import { Check } from 'lucide-react';
import { CATEGORY_COLORS, type Category, type CategoryColor } from './categories';
import './category-colors.css';

export function CategoryChip({ category, color, className = '' }: { category?: Category; color?: CategoryColor; className?: string }) {
  return <span className={`category-chip ${className}`} data-category-color={color}><span>{category?.name || '未分类'}</span></span>;
}

export function CategoryColorPicker({ value, onChange, disabled = false, label = '分类颜色' }: { value: CategoryColor; onChange: (color: CategoryColor) => void; disabled?: boolean; label?: string }) {
  return <fieldset className="category-color-picker" disabled={disabled}><legend>{label}</legend><div>{CATEGORY_COLORS.map(color => <button type="button" key={color.value} className="category-color-option" data-category-color={color.value} aria-label={color.label} aria-pressed={value === color.value} title={color.label} onClick={() => onChange(color.value)}><span className="category-color-dot" aria-hidden="true" />{color.label}{value === color.value && <Check size={14} aria-hidden="true" />}</button>)}</div></fieldset>;
}
