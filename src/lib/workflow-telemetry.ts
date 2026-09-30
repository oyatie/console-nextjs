// workflow-telemetry.ts — Enterprise Workflow Interaction & Action Counter Engine
// Instruments business workflows, measures action counts (clicks, keystrokes, submits),
// tracks screen and state transitions, and records BAU vs Edge Case paths.

export type WorkflowActionType =
  | "click"
  | "input"
  | "keypress"
  | "tab_switch"
  | "modal_open"
  | "modal_close"
  | "drawer_open"
  | "drawer_close"
  | "paste"
  | "submit";

export interface WorkflowActionRecord {
  step: number;
  type: WorkflowActionType;
  target: string;
  payload?: any;
  timestamp: string;
}

export interface WorkflowStateTransition {
  step: number;
  fromState: string;
  toState: string;
  trigger: string;
  stateDiff?: Record<string, { before: any; after: any }>;
  timestamp: string;
}

export interface WorkflowScreenChange {
  step: number;
  fromScreen: string;
  toScreen: string;
  reason: string;
  timestamp: string;
}

export interface WorkflowEdgeCaseRecord {
  code: string;
  category: "validation_error" | "statutory_limit" | "policy_forbid" | "data_collision" | "recovery";
  description: string;
  recovered: boolean;
  recoveryAction?: string;
  timestamp: string;
}

export interface WorkflowTelemetryReport {
  workflowId: string;
  workflowName: string;
  startedAt: string;
  completedAt?: string;
  durationMs: number;
  status: "IN_PROGRESS" | "COMPLETED" | "BLOCKED" | "ABORTED";
  isBAU: boolean; // Business As Usual flag
  currentScreen: string;
  currentState: string;
  totalClicks: number;
  totalInputs: number;
  totalActions: number;
  totalStateTransitions: number;
  actions: WorkflowActionRecord[];
  stateTransitions: WorkflowStateTransition[];
  screenChanges: WorkflowScreenChange[];
  edgeCases: WorkflowEdgeCaseRecord[];
}

export class WorkflowSession {
  private report: WorkflowTelemetryReport;
  private stepCounter: number = 0;
  private startTimestamp: number;

  constructor(
    workflowId: string,
    workflowName: string,
    initialScreen: string = "root",
    initialState: string = "idle"
  ) {
    this.startTimestamp = Date.now();
    this.report = {
      workflowId,
      workflowName,
      startedAt: new Date().toISOString(),
      durationMs: 0,
      status: "IN_PROGRESS",
      isBAU: true,
      currentScreen: initialScreen,
      currentState: initialState,
      totalClicks: 0,
      totalInputs: 0,
      totalActions: 0,
      totalStateTransitions: 0,
      actions: [],
      stateTransitions: [],
      screenChanges: [],
      edgeCases: [],
    };
  }

  /** Record a user click */
  recordClick(target: string, payload?: any): this {
    this.stepCounter++;
    this.report.totalClicks++;
    this.report.totalActions++;
    this.report.actions.push({
      step: this.stepCounter,
      type: "click",
      target,
      payload,
      timestamp: new Date().toISOString(),
    });
    return this;
  }

  /** Record a form/cell input */
  recordInput(target: string, value: any): this {
    this.stepCounter++;
    this.report.totalInputs++;
    this.report.totalActions++;
    this.report.actions.push({
      step: this.stepCounter,
      type: "input",
      target,
      payload: value,
      timestamp: new Date().toISOString(),
    });
    return this;
  }

  /** Record any interaction action */
  recordAction(type: WorkflowActionType, target: string, payload?: any): this {
    this.stepCounter++;
    if (type === "click") this.report.totalClicks++;
    if (type === "input") this.report.totalInputs++;
    this.report.totalActions++;
    this.report.actions.push({
      step: this.stepCounter,
      type,
      target,
      payload,
      timestamp: new Date().toISOString(),
    });
    return this;
  }

  /** Record navigation / surface screen change */
  recordScreenChange(toScreen: string, reason: string = "user_navigation"): this {
    const fromScreen = this.report.currentScreen;
    if (fromScreen === toScreen) return this;

    this.stepCounter++;
    this.report.screenChanges.push({
      step: this.stepCounter,
      fromScreen,
      toScreen,
      reason,
      timestamp: new Date().toISOString(),
    });
    this.report.currentScreen = toScreen;
    return this;
  }

  /** Record business state machine transition */
  recordStateTransition(
    toState: string,
    trigger: string,
    stateDiff?: Record<string, { before: any; after: any }>
  ): this {
    const fromState = this.report.currentState;
    this.stepCounter++;
    this.report.totalStateTransitions++;
    this.report.stateTransitions.push({
      step: this.stepCounter,
      fromState,
      toState,
      trigger,
      stateDiff,
      timestamp: new Date().toISOString(),
    });
    this.report.currentState = toState;
    return this;
  }

  /** Record an encountered edge case or exception (breaks pure BAU status) */
  recordEdgeCase(
    code: string,
    category: WorkflowEdgeCaseRecord["category"],
    description: string,
    recovered: boolean = true,
    recoveryAction?: string
  ): this {
    this.report.isBAU = false;
    this.report.edgeCases.push({
      code,
      category,
      description,
      recovered,
      recoveryAction,
      timestamp: new Date().toISOString(),
    });
    return this;
  }

  /** Finalize and seal the workflow report */
  complete(status: "COMPLETED" | "BLOCKED" | "ABORTED" = "COMPLETED"): WorkflowTelemetryReport {
    this.report.status = status;
    this.report.completedAt = new Date().toISOString();
    this.report.durationMs = Date.now() - this.startTimestamp;
    return { ...this.report };
  }

  getReport(): WorkflowTelemetryReport {
    return { ...this.report };
  }
}

/** Global Workflow Telemetry Registry for in-flight and completed sessions */
class WorkflowTelemetryRegistry {
  private sessions: Map<string, WorkflowTelemetryReport> = new Map();

  createSession(
    workflowId: string,
    workflowName: string,
    initialScreen?: string,
    initialState?: string
  ): WorkflowSession {
    const session = new WorkflowSession(workflowId, workflowName, initialScreen, initialState);
    return session;
  }

  recordCompleted(report: WorkflowTelemetryReport) {
    this.sessions.set(report.workflowId, report);
  }

  getSession(workflowId: string): WorkflowTelemetryReport | undefined {
    return this.sessions.get(workflowId);
  }

  getAllSessions(): WorkflowTelemetryReport[] {
    return Array.from(this.sessions.values());
  }

  clear() {
    this.sessions.clear();
  }
}

export const workflowTelemetry = new WorkflowTelemetryRegistry();
