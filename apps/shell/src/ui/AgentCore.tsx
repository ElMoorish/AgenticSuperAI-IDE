import { Activity, Compass, Droplet, Flame, Sparkles, Zap, type LucideIcon } from "lucide-react";
import { useStore } from "../state/store";
import type { ThemeName } from "../styles/themes";

/** Each theme has its own symbol for "the model is here". It pulses while the agent works. */
const ICONS: Record<ThemeName, LucideIcon> = {
  voltage: Zap,
  forge: Flame,
  aurora: Sparkles,
  drafting: Compass,
  deepsea: Droplet,
  monolith: Activity,
};

export function AgentCore({ busy }: { busy: boolean }) {
  const theme = useStore((s) => s.theme);
  const Icon = ICONS[theme];
  return (
    <span className={`core core-${theme} ${busy ? "busy" : "idle"}`} aria-hidden="true">
      <Icon size={24} strokeWidth={1.8} />
    </span>
  );
}
