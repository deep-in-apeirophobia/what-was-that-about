import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import clsx from "clsx";
import { Loader2 } from "lucide-react";

type Variant = "primary" | "secondary" | "ghost" | "danger" | "danger-ghost" | "soft";
type Size = "sm" | "md" | "lg";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent text-accent-fg hover:bg-accent-hover shadow-sm",
  secondary: "bg-surface text-fg border border-line hover:bg-hover hover:border-line-strong",
  ghost: "text-muted hover:text-fg hover:bg-hover",
  danger: "bg-danger text-white hover:brightness-110 shadow-sm",
  "danger-ghost": "text-danger hover:bg-danger-soft",
  soft: "bg-accent-soft text-accent hover:brightness-95 dark:hover:brightness-125",
};

const SIZES: Record<Size, string> = {
  sm: "h-7 px-2.5 text-[12.5px] gap-1.5 rounded-md",
  md: "h-8 px-3 text-[13px] gap-1.5 rounded-lg",
  lg: "h-10 px-4 text-sm gap-2 rounded-lg",
};

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  icon?: ReactNode;
  loading?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = "secondary", size = "md", icon, loading, className, children, disabled, type = "button", ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type}
      disabled={disabled || loading}
      className={clsx(
        "inline-flex items-center justify-center font-medium whitespace-nowrap select-none transition-[background-color,border-color,color,filter] disabled:opacity-50 disabled:pointer-events-none",
        VARIANTS[variant],
        SIZES[size],
        className,
      )}
      {...rest}
    >
      {loading ? <Loader2 className="size-3.5 animate-spin" aria-hidden /> : icon}
      {children}
    </button>
  );
});

export interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  label: string;
  size?: "xs" | "sm" | "md" | "lg";
  variant?: "ghost" | "secondary" | "primary" | "danger-ghost";
  active?: boolean;
}

const ICON_SIZES = { xs: "size-6 rounded-md", sm: "size-7 rounded-md", md: "size-8 rounded-lg", lg: "size-10 rounded-full" };
const ICON_VARIANTS = {
  ghost: "text-muted hover:text-fg hover:bg-hover",
  secondary: "text-fg bg-surface border border-line hover:bg-hover",
  primary: "bg-accent text-accent-fg hover:bg-accent-hover shadow-sm",
  "danger-ghost": "text-muted hover:text-danger hover:bg-danger-soft",
};

/** Icon-only button. `label` becomes the aria-label and tooltip. */
export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { label, size = "sm", variant = "ghost", active, className, children, type = "button", title, ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type}
      aria-label={label}
      title={title ?? label}
      aria-pressed={active}
      className={clsx(
        "inline-flex items-center justify-center shrink-0 transition-colors disabled:opacity-40 disabled:pointer-events-none",
        ICON_SIZES[size],
        ICON_VARIANTS[variant],
        active && "!text-accent bg-accent-soft",
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
});
