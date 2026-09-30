"use client";

import React, { useEffect, useMemo } from "react";
import { useAppStore } from "@/lib/store";
import {
  Building2,
  MapPin,
  Users2,
  Lock,
  Globe2,
  Filter,
  CheckCircle2,
  ShieldAlert,
} from "lucide-react";

export interface ModuleScopeState {
  entityId: string; // "all" or entity id/name
  siteId: string; // "all" or site name
  deptId: string; // "all" or dept name
}

export interface ModuleScopeFilterProps {
  value: ModuleScopeState;
  onChange: (next: ModuleScopeState) => void;
  filteredCount?: number;
  totalCount?: number;
  showSite?: boolean;
  showDept?: boolean;
  className?: string;
}

export function ModuleScopeFilter({
  value,
  onChange,
  filteredCount,
  totalCount,
  showSite = true,
  showDept = true,
  className = "",
}: ModuleScopeFilterProps) {
  const viewAsRole = useAppStore((s) => s.viewAsRole);
  const employees = useAppStore((s) => s.employees);
  const roleAssignments = useAppStore((s) => s.roleAssignments);
  const roleDefinitions = useAppStore((s) => s.roleDefinitions);
  const orgEntities = useAppStore((s) => s.orgEntities);
  const orgSites = useAppStore((s) => s.orgSites);
  const orgDepartments = useAppStore((s) => s.orgDepartments);

  // Active Person Identity
  const currentPerson =
    employees.find((e) => e.role === viewAsRole || e.grade === viewAsRole) ||
    employees[1] ||
    employees[0];

  // Effective Role Assignments for Current Person
  const currentAssignments = useMemo(() => {
    return roleAssignments.filter(
      (a) =>
        (a.employeeId === currentPerson?.id || a.personId === `P-${currentPerson?.id}`) &&
        a.status === "active"
    );
  }, [roleAssignments, currentPerson]);

  // Determine Maximum Permitted Scope from folded roles
  const policyScope = useMemo(() => {
    // 1. If any role has "all" scope, user has full group-wide cross-corporation access
    const hasAll = currentAssignments.some((a) => a.scope.type === "all");
    if (hasAll) {
      return {
        type: "all" as const,
        entityName: null,
        siteName: null,
        deptName: null,
        label: "전사 총괄 인가 (Cross-Corporation)",
        isRestricted: false,
      };
    }

    // 2. Check for site-level restriction (e.g. 현장 소장, 정비수석)
    const siteAssign = currentAssignments.find((a) => a.scope.type === "site");
    if (siteAssign) {
      const siteName = siteAssign.scope.targetName || siteAssign.scope.targetId || "인천 제1물류센터";
      const matchedSite = orgSites.find((s) => s.name === siteName || s.id === siteName);
      const matchedEntity = orgEntities.find((e) => e.id === matchedSite?.entityId);
      return {
        type: "site" as const,
        entityName: matchedEntity?.name || "(주)오야티 로지스틱스",
        siteName: siteName,
        deptName: null,
        label: `현장 관할 한정: ${siteName}`,
        isRestricted: true,
      };
    }

    // 3. Check for entity-level restriction
    const entityAssign = currentAssignments.find((a) => a.scope.type === "entity");
    if (entityAssign) {
      const entName = entityAssign.scope.targetName || entityAssign.scope.targetId || "(주)오야티 로지스틱스";
      return {
        type: "entity" as const,
        entityName: entName,
        siteName: null,
        deptName: null,
        label: `법인 관할 한정: ${entName}`,
        isRestricted: true,
      };
    }

    // 4. Default: department or self boundary based on employee's own placement
    return {
      type: "department" as const,
      entityName: currentPerson?.entity || "(주)오야티 코퍼레이션",
      siteName: currentPerson?.site || "서울 본사 타워",
      deptName: currentPerson?.dept || "인사노무팀",
      label: `부서 관할 한정: ${currentPerson?.dept || "인사노무팀"}`,
      isRestricted: true,
    };
  }, [currentAssignments, currentPerson]);

  // Enforce policy clamps: if persona changes or current value violates policy scope, clamp it immediately
  useEffect(() => {
    let nextEntity = value.entityId;
    let nextSite = value.siteId;
    let nextDept = value.deptId;
    let changed = false;

    if (policyScope.type === "site") {
      if (nextEntity !== policyScope.entityName) {
        nextEntity = policyScope.entityName || "all";
        changed = true;
      }
      if (nextSite !== policyScope.siteName) {
        nextSite = policyScope.siteName || "all";
        changed = true;
      }
    } else if (policyScope.type === "entity") {
      if (nextEntity !== policyScope.entityName) {
        nextEntity = policyScope.entityName || "all";
        changed = true;
      }
    }

    if (changed) {
      onChange({ entityId: nextEntity, siteId: nextSite, deptId: nextDept });
    }
  }, [policyScope, value, onChange]);

  // Available options based on policy scope
  const availableEntities = useMemo(() => {
    if (policyScope.type === "all") {
      return [{ id: "all", name: "전체 법인 (All Group Entities)" }, ...orgEntities];
    }
    const matched = orgEntities.find(
      (e) => e.name === policyScope.entityName || e.id === policyScope.entityName
    );
    return matched ? [matched] : [{ id: policyScope.entityName || "corp", name: policyScope.entityName || "소속 법인" }];
  }, [policyScope, orgEntities]);

  const availableSites = useMemo(() => {
    if (policyScope.type === "site") {
      return [{ id: policyScope.siteName, name: policyScope.siteName }];
    }
    let list = orgSites;
    if (value.entityId !== "all") {
      const ent = orgEntities.find((e) => e.name === value.entityId || e.id === value.entityId);
      list = list.filter((s) => s.entityId === ent?.id || s.entityId === value.entityId);
    }
    return [{ id: "all", name: "전체 현장/사업장" }, ...list];
  }, [policyScope, value.entityId, orgSites, orgEntities]);

  const availableDepts = useMemo(() => {
    let list = orgDepartments;
    if (value.entityId !== "all") {
      const ent = orgEntities.find((e) => e.name === value.entityId || e.id === value.entityId);
      list = list.filter((d) => d.entityId === ent?.id || d.entityId === value.entityId);
    }
    return [{ id: "all", name: "전체 부서" }, ...list];
  }, [value.entityId, orgDepartments, orgEntities]);

  return (
    <div
      className={`flex flex-wrap items-center justify-between gap-3 p-2.5 rounded-lg border border-[var(--border)] bg-[var(--surface)] text-xs shadow-2xs ${className}`}
    >
      {/* 좌측: 모듈 관할 필터 드롭다운 컨트롤 */}
      <div className="flex flex-wrap items-center gap-2">
        <div className="flex items-center gap-1 text-[var(--steel)] font-semibold pr-1 border-r border-[var(--border)]">
          <Filter size={13} className="text-amber-600" />
          <span>관할 필터:</span>
        </div>

        {/* 법인 선택 (Cross-Corp vs Policy Locked) */}
        <div className="relative flex items-center">
          <Building2 size={13} className="absolute left-2 text-[var(--faint)] pointer-events-none" />
          <select
            value={value.entityId}
            disabled={policyScope.type !== "all"}
            onChange={(e) =>
              onChange({
                ...value,
                entityId: e.target.value,
                siteId: "all",
                deptId: "all",
              })
            }
            className={`pl-6.5 pr-2.5 py-1 text-xs rounded border ${
              policyScope.type !== "all"
                ? "border-amber-300 bg-amber-50/60 text-amber-950 font-bold cursor-not-allowed"
                : "border-[var(--border)] bg-[var(--canvas)] text-[var(--ink)] cursor-pointer hover:border-gray-400"
            } focus:outline-none`}
            title={policyScope.type !== "all" ? "Cedar 보안 정책에 의해 소속 법인으로 고정되었습니다." : "조회할 법인을 선택하십시오."}
          >
            {availableEntities.map((ent) => (
              <option key={ent.id} value={ent.id === "all" ? "all" : ent.name}>
                {ent.name}
              </option>
            ))}
          </select>
          {policyScope.type !== "all" && (
            <Lock size={10} className="absolute right-1.5 text-amber-600 pointer-events-none" />
          )}
        </div>

        {/* 사업장 / 현장 선택 */}
        {showSite && (
          <div className="relative flex items-center">
            <MapPin size={13} className="absolute left-2 text-[var(--faint)] pointer-events-none" />
            <select
              value={value.siteId}
              disabled={policyScope.type === "site"}
              onChange={(e) => onChange({ ...value, siteId: e.target.value })}
              className={`pl-6.5 pr-2.5 py-1 text-xs rounded border ${
                policyScope.type === "site"
                  ? "border-amber-300 bg-amber-50/60 text-amber-950 font-bold cursor-not-allowed"
                  : "border-[var(--border)] bg-[var(--canvas)] text-[var(--ink)] cursor-pointer hover:border-gray-400"
              } focus:outline-none`}
              title={policyScope.type === "site" ? "Cedar 보안 정책에 의해 소속 현장으로 고정되었습니다." : "조회할 사업장/현장을 선택하십시오."}
            >
              {availableSites.map((s) => (
                <option key={s.id || s.name} value={s.id === "all" ? "all" : s.name}>
                  {s.name}
                </option>
              ))}
            </select>
            {policyScope.type === "site" && (
              <Lock size={10} className="absolute right-1.5 text-amber-600 pointer-events-none" />
            )}
          </div>
        )}

        {/* 부서 선택 */}
        {showDept && policyScope.type !== "site" && (
          <div className="relative flex items-center">
            <Users2 size={13} className="absolute left-2 text-[var(--faint)] pointer-events-none" />
            <select
              value={value.deptId}
              onChange={(e) => onChange({ ...value, deptId: e.target.value })}
              className="pl-6.5 pr-2.5 py-1 text-xs rounded border border-[var(--border)] bg-[var(--canvas)] text-[var(--ink)] cursor-pointer hover:border-gray-400 focus:outline-none"
            >
              {availableDepts.map((d) => (
                <option key={d.id || d.name} value={d.id === "all" ? "all" : d.name}>
                  {d.name}
                </option>
              ))}
            </select>
          </div>
        )}
      </div>

      {/* 우측: 인가 거버넌스 상태 뱃지 & 실시간 카운터 */}
      <div className="flex items-center gap-2.5">
        <div
          className={`flex items-center gap-1.5 px-2 py-0.5 rounded text-[11px] font-medium ${
            policyScope.isRestricted
              ? "bg-amber-100 text-amber-900 border border-amber-300"
              : "bg-blue-50 text-blue-900 border border-blue-200"
          }`}
        >
          {policyScope.isRestricted ? (
            <Lock size={11} className="text-amber-700" />
          ) : (
            <Globe2 size={11} className="text-blue-600" />
          )}
          <span>{policyScope.label}</span>
        </div>

        {filteredCount !== undefined && totalCount !== undefined && (
          <span className="text-[11px] text-[var(--steel)] font-mono">
            표시: <strong className="text-[var(--ink)]">{filteredCount}</strong> / {totalCount}
          </span>
        )}
      </div>
    </div>
  );
}
