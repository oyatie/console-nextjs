"use client";

import React, { forwardRef, ButtonHTMLAttributes } from "react";
import { clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: "primary" | "secondary" | "outline" | "ghost" | "danger" | "brand";
  size?: "xs" | "sm" | "md" | "lg";
  loading?: boolean;
  leftIcon?: React.ReactNode;
  rightIcon?: React.ReactNode;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  (
    {
      className,
      variant = "secondary",
      size = "md",
      loading = false,
      disabled,
      children,
      leftIcon,
      rightIcon,
      ...props
    },
    ref
  ) => {
    const baseStyles =
      "inline-flex items-center justify-center font-medium rounded-[6px] transition-colors focus:outline-none focus:ring-2 focus:ring-[var(--signal)] focus:ring-offset-1 disabled:opacity-50 disabled:pointer-events-none select-none";

    const variants = {
      primary: "bg-[var(--ink)] text-white hover:bg-black/90 active:scale-[0.99]",
      brand: "bg-[var(--signal)] text-[var(--ink)] font-semibold hover:bg-[var(--signal-deep)] shadow-sm active:scale-[0.99]",
      secondary: "bg-[var(--surface)] text-[var(--ink)] border border-[var(--border)] hover:bg-[var(--muted)] hover:border-[var(--steel)] shadow-xs",
      outline: "border border-[var(--border)] text-[var(--ink)] bg-transparent hover:bg-[var(--muted)]",
      ghost: "text-[var(--steel)] hover:text-[var(--ink)] hover:bg-[var(--muted)]",
      danger: "bg-[var(--danger-bg)] text-[var(--danger-tx)] border border-[var(--danger-bd)] hover:bg-red-100",
    };

    const sizes = {
      xs: "text-xs px-2 py-0.5 h-6 gap-1",
      sm: "text-xs px-2.5 py-1 h-7 gap-1.5",
      md: "text-sm px-3.5 py-1.5 h-8 gap-2",
      lg: "text-sm px-4 py-2 h-9 gap-2",
    };

    return (
      <button
        ref={ref}
        disabled={disabled || loading}
        className={twMerge(clsx(baseStyles, variants[variant], sizes[size], className))}
        {...props}
      >
        {loading ? (
          <svg className="animate-spin h-3.5 w-3.5" viewBox="0 0 24 24" fill="none">
            <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
            <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8H4z" />
          </svg>
        ) : (
          leftIcon
        )}
        {children}
        {!loading && rightIcon}
      </button>
    );
  }
);

Button.displayName = "Button";
