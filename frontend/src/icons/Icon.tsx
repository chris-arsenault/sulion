import type { ComponentType, SVGProps } from "react";
import {
  Activity,
  AlertTriangle,
  Check,
  ChevronDown,
  ChevronRight,
  Clock,
  Code2,
  Command,
  Copy,
  Cpu,
  Diff,
  Eye,
  File as FileIcon,
  FileText,
  Folder,
  FolderOpen,
  GitBranch,
  GitCommit,
  Hammer,
  Info,
  Layers,
  Link as LinkIcon,
  List,
  ListChecks,
  Menu,
  MoreHorizontal,
  PanelLeft,
  PanelLeftClose,
  Pencil,
  Pin,
  PinOff,
  Plus,
  RefreshCw,
  Search,
  Send,
  Settings,
  Sparkles,
  SquareTerminal,
  Target,
  Trash2,
  X,
} from "lucide-react";

import { Dirty } from "./sigils/Dirty";
import { Jsonl } from "./sigils/Jsonl";
import { ParentSession } from "./sigils/ParentSession";
import { PtyBound } from "./sigils/PtyBound";
import { RepoStale } from "./sigils/RepoStale";
import { SessionDead } from "./sigils/SessionDead";
import { SessionLive } from "./sigils/SessionLive";
import { SessionOrphan } from "./sigils/SessionOrphan";
import { Unread } from "./sigils/Unread";

const REGISTRY = {
  // Lucide — generic verbs/nouns
  activity: Activity,
  "alert-triangle": AlertTriangle,
  check: Check,
  "chevron-down": ChevronDown,
  "chevron-right": ChevronRight,
  clock: Clock,
  "code-2": Code2,
  command: Command,
  copy: Copy,
  cpu: Cpu,
  diff: Diff,
  eye: Eye,
  file: FileIcon,
  "file-text": FileText,
  folder: Folder,
  "folder-open": FolderOpen,
  "git-branch": GitBranch,
  "git-commit": GitCommit,
  hammer: Hammer,
  info: Info,
  layers: Layers,
  link: LinkIcon,
  list: List,
  "list-checks": ListChecks,
  menu: Menu,
  "more-horizontal": MoreHorizontal,
  "panel-left": PanelLeft,
  "panel-left-close": PanelLeftClose,
  pencil: Pencil,
  pin: Pin,
  "pin-off": PinOff,
  plus: Plus,
  "refresh-cw": RefreshCw,
  search: Search,
  send: Send,
  settings: Settings,
  sparkles: Sparkles,
  terminal: SquareTerminal,
  target: Target,
  "trash-2": Trash2,
  x: X,

  // Custom sigils
  dirty: Dirty,
  jsonl: Jsonl,
  "parent-session": ParentSession,
  "pty-bound": PtyBound,
  "repo-stale": RepoStale,
  "session-dead": SessionDead,
  "session-live": SessionLive,
  "session-orphan": SessionOrphan,
  unread: Unread,
} satisfies Record<string, ComponentType<SVGProps<SVGSVGElement>>>;

export type IconName = keyof typeof REGISTRY;
export type IconSize = 12 | 14 | 16 | 20;

interface IconProps {
  name: IconName;
  size?: IconSize;
  className?: string;
  "aria-label"?: string;
}

export function Icon({
  name,
  size = 16,
  className,
  "aria-label": ariaLabel,
}: IconProps) {
  const Glyph = REGISTRY[name];
  const ariaProps = ariaLabel
    ? { role: "img" as const, "aria-label": ariaLabel }
    : { "aria-hidden": true };
  return (
    <Glyph
      width={size}
      height={size}
      className={className}
      strokeWidth={1.5}
      {...ariaProps}
    />
  );
}
