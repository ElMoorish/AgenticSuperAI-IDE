import { diffLines } from "../agent/diff";
import type { Approval } from "../agent/agentStore";

/** Messages between the IDE window and the small approval window (Tauri events, app-internal). */
export const EV_APPROVALS = "bot:approvals"; // main -> overlay: the requests waiting right now
export const EV_READY = "bot:ready"; // overlay -> main: "send me the list"
export const EV_DECIDE = "bot:decide"; // overlay -> main: { id, ok }
export const EV_OPEN = "bot:open"; // overlay -> main: bring the IDE to the front

export interface BotItem {
  id: number;
  kind: "edit" | "command";
  /** File path (edit) or tool title (command). */
  title: string;
  command?: string;
  /** `+2 −1` for an edit; omitted for commands. */
  stat?: string;
  isNew?: boolean;
}

export interface Decision {
  id: number;
  ok: boolean;
}

/** The light-weight view sent to the overlay: no file contents cross windows. */
export function toBotItems(approvals: Approval[]): BotItem[] {
  return approvals.map((a) => {
    if (a.kind !== "edit") return { id: a.id, kind: "command", title: a.title, command: a.command };
    const d = diffLines(a.old ?? "", a.new ?? "");
    return {
      id: a.id,
      kind: "edit",
      title: a.title,
      stat: d.tooLarge ? "large change" : `+${d.added} −${d.removed}`,
      isNew: (a.old ?? "") === "",
    };
  });
}

/** Whether a message from the other window is a well-formed decision. */
export function parseDecision(v: unknown): Decision | null {
  if (!v || typeof v !== "object") return null;
  const { id, ok } = v as Record<string, unknown>;
  return typeof id === "number" && Number.isInteger(id) && typeof ok === "boolean" ? { id, ok } : null;
}
