import { useEffect, useRef, useState } from "react";
import { create } from "zustand";

interface ConfirmSpec {
  kind: "confirm";
  title: string;
  body?: React.ReactNode;
  confirmLabel: string;
  danger?: boolean;
  extra?: { label: string; onClick: () => void };
  onConfirm: () => void;
}
interface PromptSpec {
  kind: "prompt";
  title: string;
  label?: string;
  placeholder?: string;
  initial?: string;
  confirmLabel: string;
  onSubmit: (value: string) => void;
}

export const useDialog = create<{ spec: ConfirmSpec | PromptSpec | null }>(() => ({ spec: null }));

export function confirmDialog(spec: Omit<ConfirmSpec, "kind">) {
  useDialog.setState({ spec: { kind: "confirm", ...spec } });
}
export function promptDialog(spec: Omit<PromptSpec, "kind">) {
  useDialog.setState({ spec: { kind: "prompt", ...spec } });
}

export function Dialogs() {
  const spec = useDialog((s) => s.spec);
  const [value, setValue] = useState("");
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (spec?.kind === "prompt") {
      setValue(spec.initial ?? "");
      setTimeout(() => input.current?.select(), 0);
    }
  }, [spec]);
  if (!spec) return null;
  const close = () => useDialog.setState({ spec: null });
  const ok = () => {
    close();
    if (spec.kind === "confirm") spec.onConfirm();
    else if (value.trim()) spec.onSubmit(value.trim());
  };
  return (
    <div className="overlay" onMouseDown={close}>
      <div className="sheet small" role="dialog" aria-label={spec.title} onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") close();
          if (e.key === "Enter" && spec.kind === "prompt") ok();
        }}>
        <h3>{spec.title}</h3>
        {spec.kind === "confirm" ? (
          spec.body && <div className="muted dialog-body">{spec.body}</div>
        ) : (
          <label>{spec.label}
            <input ref={input} className="field" value={value} placeholder={spec.placeholder} onChange={(e) => setValue(e.target.value)} data-testid="prompt-input" />
          </label>
        )}
        <div className="sheet-actions">
          <button className="btn" onClick={close}>Cancel</button>
          {spec.kind === "confirm" && spec.extra && (
            <button className="btn" onClick={() => { close(); spec.extra!.onClick(); }}>{spec.extra.label}</button>
          )}
          <button className={`btn ${spec.kind === "confirm" && spec.danger ? "danger" : "primary"}`} onClick={ok} autoFocus={spec.kind === "confirm"} data-testid="dialog-ok">
            {spec.confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
