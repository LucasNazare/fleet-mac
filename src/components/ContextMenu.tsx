import { useEffect } from "react";

export interface MenuItem {
  label: string;
  onClick?: () => void;
  danger?: boolean;
  disabled?: boolean;
  swatches?: { color: string; onPick: () => void }[];
}

export function ContextMenu({ x, y, items, onClose }: { x: number; y: number; items: MenuItem[]; onClose: () => void }) {
  useEffect(() => {
    const close = () => onClose();
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [onClose]);
  const top = Math.min(y, window.innerHeight - items.length * 30 - 20);
  return (
    <div className="menu" style={{ left: x, top }} onMouseDown={(e) => e.stopPropagation()} role="menu">
      {items.map((it, i) =>
        it.swatches ? (
          <div key={i} className="menu-swatches">
            <span className="muted">{it.label}</span>
            <div>
              {it.swatches.map((s) => (
                <button key={s.color} className="swatch" style={{ background: s.color }} aria-label={`Color ${s.color}`}
                  onClick={() => { s.onPick(); onClose(); }} />
              ))}
            </div>
          </div>
        ) : (
          <button key={i} role="menuitem" className={`menu-item ${it.danger ? "danger" : ""}`} disabled={it.disabled}
            onClick={() => { it.onClick?.(); onClose(); }}>
            {it.label}
          </button>
        ),
      )}
    </div>
  );
}
