import { type ReactNode, useEffect, useId, useState } from 'react';

interface SwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: ReactNode;
  description?: ReactNode;
  disabled?: boolean;
  size?: 'normal' | 'large';
}

/** An on/off switch with a visible label (a styled checkbox with role="switch"). */
export function Switch({ checked, onChange, label, description, disabled, size = 'normal' }: SwitchProps) {
  const descriptionId = useId();
  return (
    <div className={`switch switch-${size}`} data-disabled={disabled || undefined}>
      <label className="switch-control">
        <input
          type="checkbox"
          role="switch"
          checked={checked}
          disabled={disabled}
          aria-describedby={description ? descriptionId : undefined}
          onChange={(e) => onChange(e.target.checked)}
        />
        <span className="switch-track" aria-hidden="true">
          <span className="switch-thumb" />
        </span>
        <span className="switch-label">{label}</span>
      </label>
      {description && (
        <p className="switch-description" id={descriptionId}>
          {description}
        </p>
      )}
    </div>
  );
}

interface SegmentedProps<T extends string | number> {
  value: T;
  options: readonly { value: T; label: ReactNode; title?: string }[];
  onChange: (value: T) => void;
  label: string;
  disabled?: boolean;
  wrap?: boolean;
}

/** A single-choice button group (radio semantics). */
export function Segmented<T extends string | number>({ value, options, onChange, label, disabled, wrap }: SegmentedProps<T>) {
  return (
    <div className={`segmented${wrap ? ' segmented-wrap' : ''}`} role="radiogroup" aria-label={label}>
      {options.map((option) => (
        <button
          key={String(option.value)}
          type="button"
          role="radio"
          aria-checked={option.value === value}
          title={option.title}
          disabled={disabled}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

interface SelectProps<T extends string | number> {
  value: T;
  options: readonly { value: T; label: string }[];
  onChange: (value: T) => void;
  label: ReactNode;
  disabled?: boolean;
}

export function Select<T extends string | number>({ value, options, onChange, label, disabled }: SelectProps<T>) {
  const id = useId();
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <select
        id={id}
        value={String(value)}
        disabled={disabled}
        onChange={(e) => {
          const picked = options.find((o) => String(o.value) === e.target.value);
          if (picked) onChange(picked.value);
        }}
      >
        {options.map((o) => (
          <option key={String(o.value)} value={String(o.value)}>
            {o.label}
          </option>
        ))}
      </select>
    </div>
  );
}

interface CheckboxProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  children: ReactNode;
  disabled?: boolean;
}

export function Checkbox({ checked, onChange, children, disabled }: CheckboxProps) {
  return (
    <label className="checkbox">
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>{children}</span>
    </label>
  );
}

/** A titled group of settings. */
export function Section({ title, children, aside }: { title: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <section className="settings-section">
      <header className="settings-section-header">
        <h3>{title}</h3>
        {aside}
      </header>
      {children}
    </section>
  );
}

interface NumberFieldProps {
  value: number;
  onChange: (value: number) => void;
  label: ReactNode;
  min: number;
  max: number;
  disabled?: boolean;
  hint?: ReactNode;
}

/** A whole-number input that accepts "100,000" or "1_000" and clamps on commit (blur or Enter). */
export function NumberField({ value, onChange, label, min, max, disabled, hint }: NumberFieldProps) {
  const id = useId();
  const [text, setText] = useState(() => value.toLocaleString('en-US'));
  useEffect(() => setText(value.toLocaleString('en-US')), [value]);

  const commit = () => {
    const parsed = Number(text.replace(/[\s,_]/g, ''));
    if (text.trim() === '' || !Number.isFinite(parsed)) {
      setText(value.toLocaleString('en-US'));
      return;
    }
    const next = Math.min(max, Math.max(min, Math.round(parsed)));
    setText(next.toLocaleString('en-US'));
    if (next !== value) onChange(next);
  };

  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <input
        id={id}
        type="text"
        inputMode="numeric"
        autoComplete="off"
        value={text}
        disabled={disabled}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commit();
        }}
      />
      {hint && <span className="field-hint">{hint}</span>}
    </div>
  );
}
