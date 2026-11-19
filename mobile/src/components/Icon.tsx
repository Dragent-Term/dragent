import { createContext, useContext } from "react";
import { StyleSheet, View } from "react-native";
import {
  Terminal, LayoutGrid, Settings, UserRound, Plus, ArrowRight, ArrowUpRight,
  ChevronRight, ArrowLeft, Link as LinkIcon, ScanLine, Search, Check, Ellipsis,
  Monitor, Server, Folder, X, Shield, Keyboard, Square, GitBranch, ListTodo, Bot, Globe, RefreshCw,
  GitPullRequest, Clock, MessageSquare, GitCommitHorizontal, Files, ChevronDown,
  Minus, Undo2, Upload, History, Eye, ArrowUp, Copy, Pencil, FileText, Wrench, Image as ImageIcon, AudioLines, Mic, Bell,
  Pin, PinOff, Trash2, Layers, Workflow,
} from "lucide-react-native";
import { useTheme } from "../theme";
import type { TextTone } from "./AppText";

const icons = {
  terminal: Terminal,
  grid: LayoutGrid,
  settings: Settings,
  user: UserRound,
  plus: Plus,
  "arrow-right": ArrowRight,
  "arrow-up-right": ArrowUpRight,
  "chevron-right": ChevronRight,
  "arrow-left": ArrowLeft,
  link: LinkIcon,
  scan: ScanLine,
  search: Search,
  check: Check,
  more: Ellipsis,
  monitor: Monitor,
  server: Server,
  folder: Folder,
  close: X,
  shield: Shield,
  keyboard: Keyboard,
  stop: Square,
  branch: GitBranch,
  tasks: ListTodo,
  agent: Bot,
  browser: Globe,
  refresh: RefreshCw,
  "pull-request": GitPullRequest,
  clock: Clock,
  message: MessageSquare,
  "git-commit": GitCommitHorizontal,
  files: Files,
  "chevron-down": ChevronDown,
  minus: Minus,
  undo: Undo2,
  upload: Upload,
  history: History,
  eye: Eye,
  "arrow-up": ArrowUp,
  copy: Copy,
  pencil: Pencil,
  file: FileText,
  tool: Wrench,
  image: ImageIcon,
  voice: AudioLines,
  mic: Mic,
  bell: Bell,
  pin: Pin,
  "pin-off": PinOff,
  trash: Trash2,
  layers: Layers,
  workflow: Workflow,
} as const;

export type IconName = keyof typeof icons;
export type IconTone = TextTone;

export interface IconProps {
  name: IconName;
  size?: number;
  tone?: IconTone;
}

// Button slots inherit their label color; explicit icon tones still take priority.
export const IconToneContext = createContext<IconTone>("textMuted");

/** Decorative by design. The surrounding action supplies its accessible label. */
export function Icon({ name, size = 18, tone }: IconProps) {
  const { theme } = useTheme();
  const inheritedTone = useContext(IconToneContext);
  const Glyph = icons[name];
  return (
    <View
      accessible={false}
      aria-hidden
      accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants"
      pointerEvents="none"
      style={styles.shell}
    >
      <Glyph size={size} strokeWidth={1.75} color={theme.colors[tone ?? inheritedTone]} focusable={false} />
    </View>
  );
}

const styles = StyleSheet.create({
  shell: { flexShrink: 0, alignItems: "center", justifyContent: "center" },
});
