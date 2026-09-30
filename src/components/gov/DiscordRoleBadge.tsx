"use client";

import React from "react";
import { RoleDefinition } from "@/lib/types";
import {
  ShieldAlert,
  KeyRound,
  ShieldCheck,
  DollarSign,
  FileCheck2,
  Users,
  FileSpreadsheet,
  Wrench,
  Building,
  User,
  Shield,
  X,
} from "lucide-react";

interface DiscordRoleBadgeProps {
  role: RoleDefinition;
  scopeLabel?: string;
  size?: "sm" | "md" | "lg";
  showPriority?: boolean;
  onRemove?: () => void;
  onClick?: () => void;
  className?: string;
}

const ICON_MAP: Record<string, React.ElementType> = {
  ShieldAlert,
  KeyRound,
  ShieldCheck,
  DollarSign,
  FileCheck2,
  Users,
  FileSpreadsheet,
  Wrench,
  Building,
  User,
  Shield,
};

export function DiscordRoleBadge({
  role,
  scopeLabel,
  size = "md",
  showPriority = false,
  onRemove,
  onClick,
  className = "",
}: DiscordRoleBadgeProps) {
  const IconComponent = (role.iconName && ICON_MAP[role.iconName]) || Shield;

  const sizeClasses = {
    sm: "text-[11px] px-2 py-0.5 gap-1.5",
    md: "text-xs px-2.5 py-1 gap-1.5",
    lg: "text-sm px-3 py-1.5 gap-2",
  };

  const iconSizes = {
    sm: 11,
    md: 13,
    lg: 15,
  };

  return (
    <span
      onClick={onClick}
      style={{
        borderColor: `${role.color}40`,
        backgroundColor: `${role.color}15`,
      }}
      className={`inline-flex items-center rounded-full font-medium border text-slate-800 dark:text-slate-100 transition-all select-none ${
        onClick ? "cursor-pointer hover:brightness-110 hover:shadow-xs" : ""
      } ${sizeClasses[size]} ${className}`}
      title={`${role.name} (우선순위: ${role.priority}, 권한 ${role.grantedCapabilities.length}건)`}
    >
      {/* Discord colored circle dot */}
      <span
        className="rounded-full shrink-0 shadow-xs"
        style={{
          backgroundColor: role.color,
          width: size === "sm" ? 7 : size === "md" ? 8 : 10,
          height: size === "sm" ? 7 : size === "md" ? 8 : 10,
        }}
      />

      {/* Role Icon */}
      <IconComponent
        size={iconSizes[size]}
        style={{ color: role.color }}
        className="shrink-0"
      />

      {/* Role Name */}
      <span className="truncate max-w-[150px]">{role.name}</span>

      {/* Optional Priority Tag */}
      {showPriority && (
        <span
          className="text-[10px] font-mono px-1 rounded bg-black/10 dark:bg-white/10 text-slate-500 dark:text-slate-400"
          title={`우선순위 계층 레벨 ${role.priority}`}
        >
          #{role.priority}
        </span>
      )}

      {/* Scoped tag if present */}
      {scopeLabel && (
        <span className="text-[10px] px-1 rounded bg-black/5 dark:bg-white/10 text-slate-600 dark:text-slate-300 font-normal">
          {scopeLabel}
        </span>
      )}

      {/* Interactive Unassign / Remove Button */}
      {onRemove && (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onRemove();
          }}
          className="ml-0.5 rounded-full p-0.5 text-slate-400 hover:text-rose-500 hover:bg-rose-50 dark:hover:bg-rose-950/40 transition-colors"
          title="이 역할 회수"
        >
          <X size={12} />
        </button>
      )}
    </span>
  );
}
