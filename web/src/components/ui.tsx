import { useEffect, useState, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode, type SelectHTMLAttributes } from "react";
import { createPortal } from "react-dom";
import { Icons } from "./icons";

export function cx(...parts: (string | false | null | undefined)[]): string {
  return parts.filter(Boolean).join(" ");
}

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "secondary" | "danger" | "ghost";
  size?: "sm" | "md";
  busy?: boolean;
};

export function Button({ variant = "secondary", size = "md", busy, className, children, disabled, ...rest }: ButtonProps) {
  const styles = {
    primary: "bg-brand-600 text-white hover:bg-brand-700 shadow-sm",
    secondary:
      "bg-white text-slate-800 ring-1 ring-inset ring-slate-300 hover:bg-slate-50 dark:bg-slate-800 dark:text-slate-100 dark:ring-slate-600 dark:hover:bg-slate-700",
    danger: "bg-red-600 text-white hover:bg-red-700 shadow-sm",
    ghost: "text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800",
  }[variant];
  return (
    <button
      type="button"
      className={cx(
        "inline-flex items-center justify-center gap-1.5 rounded-lg font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50",
        size === "sm" ? "px-2.5 py-1 text-xs" : "px-3.5 py-2 text-sm",
        styles,
        className,
      )}
      disabled={disabled || busy}
      {...rest}
    >
      {busy && <Spinner className="size-4" />}
      {children}
    </button>
  );
}

export function Spinner({ className = "size-5" }: { className?: string }) {
  return (
    <svg className={cx("animate-spin", className)} viewBox="0 0 24 24" fill="none" aria-label="Loading">
      <circle cx="12" cy="12" r="9" stroke="currentColor" strokeWidth="3" opacity="0.25" />
      <path d="M21 12a9 9 0 0 0-9-9" stroke="currentColor" strokeWidth="3" strokeLinecap="round" />
    </svg>
  );
}

export function Card({ title, actions, children, className }: { title?: ReactNode; actions?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={cx("rounded-xl bg-white shadow-sm ring-1 ring-slate-200 dark:bg-slate-900 dark:ring-slate-800", className)}>
      {(title || actions) && (
        <header className="flex items-center justify-between gap-3 border-b border-slate-100 px-5 py-3 dark:border-slate-800">
          <h2 className="text-sm font-semibold text-slate-900 dark:text-slate-100">{title}</h2>
          <div className="flex items-center gap-2">{actions}</div>
        </header>
      )}
      <div className="p-5">{children}</div>
    </section>
  );
}

/// The top of a page.  The page's name is not shown (the highlighted
/// navigation entry says it; the narrow layout's header shows it): it
/// becomes the document title and a heading for screen readers.  Page-level
/// buttons, if any, form a right-aligned row.
export function PageHeader({ title, actions }: { title: string; actions?: ReactNode }) {
  useEffect(() => {
    document.title = `${title} · Open Desk Phone`;
    return () => {
      document.title = "Open Desk Phone";
    };
  }, [title]);
  return (
    <>
      <h1 className="sr-only">{title}</h1>
      {actions && <div className="mb-5 flex flex-wrap items-center justify-end gap-2 empty:hidden">{actions}</div>}
    </>
  );
}

type Tone = "gray" | "green" | "red" | "amber" | "blue";

export function Badge({ tone = "gray", children }: { tone?: Tone; children: ReactNode }) {
  const styles: Record<Tone, string> = {
    gray: "bg-slate-100 text-slate-700 dark:bg-slate-800 dark:text-slate-300",
    green: "bg-emerald-100 text-emerald-800 dark:bg-emerald-900/50 dark:text-emerald-300",
    red: "bg-red-100 text-red-800 dark:bg-red-900/50 dark:text-red-300",
    amber: "bg-amber-100 text-amber-800 dark:bg-amber-900/50 dark:text-amber-300",
    blue: "bg-sky-100 text-sky-800 dark:bg-sky-900/50 dark:text-sky-300",
  };
  return <span className={cx("inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium", styles[tone])}>{children}</span>;
}

export function Dot({ tone }: { tone: Tone }) {
  const styles: Record<Tone, string> = {
    gray: "bg-slate-400",
    green: "bg-emerald-500",
    red: "bg-red-500",
    amber: "bg-amber-500",
    blue: "bg-sky-500",
  };
  return <span className={cx("inline-block size-2 rounded-full", styles[tone])} />;
}

export function Alert({ tone = "amber", title, children, action }: { tone?: "amber" | "red" | "blue" | "green"; title?: string; children?: ReactNode; action?: ReactNode }) {
  const styles = {
    amber: "bg-amber-50 text-amber-900 ring-amber-200 dark:bg-amber-950/40 dark:text-amber-200 dark:ring-amber-900",
    red: "bg-red-50 text-red-900 ring-red-200 dark:bg-red-950/40 dark:text-red-200 dark:ring-red-900",
    blue: "bg-sky-50 text-sky-900 ring-sky-200 dark:bg-sky-950/40 dark:text-sky-200 dark:ring-sky-900",
    green: "bg-emerald-50 text-emerald-900 ring-emerald-200 dark:bg-emerald-950/40 dark:text-emerald-200 dark:ring-emerald-900",
  }[tone];
  return (
    <div role="alert" className={cx("flex items-center gap-3 rounded-lg px-4 py-3 text-sm ring-1 ring-inset", styles)}>
      {tone !== "green" && tone !== "blue" && <Icons.warn className="size-4 shrink-0 self-start mt-0.5" />}
      <div className="min-w-0 flex-1">
        {title && <p className="font-semibold">{title}</p>}
        {children && <div className={title ? "mt-0.5" : ""}>{children}</div>}
      </div>
      {action}
    </div>
  );
}

const inputStyle =
  "block w-full rounded-lg border-0 bg-white px-3 py-2 text-sm text-slate-900 shadow-sm ring-1 ring-inset ring-slate-300 placeholder:text-slate-400 focus:ring-2 focus:ring-brand-500 disabled:bg-slate-100 disabled:text-slate-500 dark:bg-slate-800 dark:text-slate-100 dark:ring-slate-600 dark:disabled:bg-slate-800/50";

export function Input({ className, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  return <input className={cx(inputStyle, className)} {...rest} />;
}

export function Select({ className, children, ...rest }: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select className={cx(inputStyle, "pr-8", className)} {...rest}>
      {children}
    </select>
  );
}

export function Field({ label, hint, children, className }: { label: string; hint?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <label className={cx("block", className)}>
      <span className="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{label}</span>
      {children}
      {hint && <span className="mt-1 block text-xs text-slate-500 dark:text-slate-400">{hint}</span>}
    </label>
  );
}

export function Toggle({ checked, onChange, label, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <label className={cx("flex items-center gap-3 text-sm", disabled && "opacity-50")}>
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        disabled={disabled}
        onClick={() => onChange(!checked)}
        className={cx(
          "relative inline-flex h-6 w-11 shrink-0 rounded-full transition-colors",
          // Gray when it cannot be operated, darker when on.
          disabled ? (checked ? "bg-slate-500 dark:bg-slate-400" : "bg-slate-300 dark:bg-slate-600") : checked ? "bg-brand-600" : "bg-slate-300 dark:bg-slate-600",
        )}
      >
        <span className={cx("absolute top-0.5 left-0.5 size-5 rounded-full bg-white shadow transition-transform", checked && "translate-x-5")} />
      </button>
      <span className="text-slate-700 dark:text-slate-300">{label}</span>
    </label>
  );
}

/// This element wraps a control whose value the provisioning server set,
/// and carries the control's tooltip, since a disabled control gets no
/// hover.  The caller disables the control when it cannot be changed, and
/// the pointer then passes through the control to this element.
export function Dpma({ on, children, className }: { on: boolean | undefined; children: ReactNode; className?: string }) {
  return (
    <div title={on ? "Configured via DPMA" : undefined} className={cx(className, on && "[&_:disabled]:pointer-events-none")}>
      {children}
    </div>
  );
}

export function KeyValue({ rows }: { rows: [string, ReactNode][] }) {
  return (
    <dl className="grid grid-cols-[minmax(7rem,auto)_1fr] gap-x-4 gap-y-2 text-sm">
      {rows.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="text-slate-500 dark:text-slate-400">{k}</dt>
          <dd className="min-w-0 break-words font-medium text-slate-900 dark:text-slate-100">{v || <span className="font-normal text-slate-400">—</span>}</dd>
        </div>
      ))}
    </dl>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <p className="py-6 text-center text-sm text-slate-500 dark:text-slate-400">{children}</p>;
}

export function Modal({ title, children, onClose, footer }: { title: string; children: ReactNode; onClose: () => void; footer?: ReactNode }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  // In the body, not where the opener is: an ancestor with a backdrop
  // filter (the top bar) would otherwise confine the fixed overlay.
  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/50 p-4" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div role="dialog" aria-modal="true" aria-label={title} className="max-h-full w-full max-w-xl overflow-y-auto rounded-xl bg-white shadow-xl ring-1 ring-slate-200 dark:bg-slate-900 dark:ring-slate-700">
        <header className="flex items-center justify-between border-b border-slate-100 px-5 py-3 dark:border-slate-800">
          <h2 className="font-semibold">{title}</h2>
          <button type="button" onClick={onClose} aria-label="Close" className="rounded p-1 text-slate-500 hover:bg-slate-100 dark:hover:bg-slate-800">
            <Icons.x />
          </button>
        </header>
        {children && <div className="p-5">{children}</div>}
        {footer && <footer className="flex justify-end gap-2 border-t border-slate-100 px-5 py-3 dark:border-slate-800">{footer}</footer>}
      </div>
    </div>,
    document.body,
  );
}

/// A button that asks before acting.
export function ConfirmButton({
  label,
  title,
  message,
  confirmLabel,
  onConfirm,
  variant = "danger",
  requireText,
  size,
}: {
  label: ReactNode;
  title: string;
  message?: ReactNode;
  confirmLabel: string;
  onConfirm: () => Promise<unknown> | void;
  variant?: "danger" | "primary" | "secondary";
  requireText?: string;
  size?: "sm" | "md";
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [typed, setTyped] = useState("");
  const [error, setError] = useState("");
  const close = () => {
    setOpen(false);
    setTyped("");
    setError("");
  };
  return (
    <>
      <Button variant={variant === "primary" ? "primary" : variant} size={size} onClick={() => setOpen(true)}>
        {label}
      </Button>
      {open && (
        <Modal
          title={title}
          onClose={close}
          footer={
            <>
              <Button onClick={close}>Cancel</Button>
              <Button
                variant={variant === "secondary" ? "primary" : variant}
                busy={busy}
                disabled={requireText !== undefined && typed.trim().toUpperCase() !== requireText.toUpperCase()}
                onClick={async () => {
                  setBusy(true);
                  setError("");
                  try {
                    await onConfirm();
                    close();
                  } catch (e) {
                    setError(e instanceof Error ? e.message : String(e));
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {confirmLabel}
              </Button>
            </>
          }
        >
          {(message || requireText !== undefined || error) && (
            <div className="space-y-3 text-sm text-slate-700 dark:text-slate-300">
              {message && <div>{message}</div>}
              {requireText !== undefined && (
                <Field label={`Type ${requireText} to confirm`}>
                  <Input value={typed} onChange={(e) => setTyped(e.target.value)} autoFocus />
                </Field>
              )}
              {error && <Alert tone="red">{error}</Alert>}
            </div>
          )}
        </Modal>
      )}
    </>
  );
}

export const th = "px-3 py-2 text-left text-xs font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400";
export const td = "px-3 py-2 text-sm";
