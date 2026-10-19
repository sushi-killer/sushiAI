import {
  Bot,
  Box,
  Check,
  Circle,
  Cloud,
  Code,
  Cpu,
  Database,
  Diamond,
  File,
  Flag,
  Folder,
  GitBranch,
  Globe,
  KeyRound,
  Laptop,
  Lock,
  MessageSquare,
  Play,
  Puzzle,
  RectangleHorizontal,
  Server,
  Smartphone,
  Square,
  Terminal,
  User,
  Users,
  Zap,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { IconName, Shape } from "./types.ts";

export const ICON_OF: Record<IconName, LucideIcon> = {
  laptop: Laptop,
  server: Server,
  database: Database,
  cloud: Cloud,
  phone: Smartphone,
  globe: Globe,
  bot: Bot,
  user: User,
  users: Users,
  terminal: Terminal,
  code: Code,
  puzzle: Puzzle,
  file: File,
  folder: Folder,
  message: MessageSquare,
  key: KeyRound,
  lock: Lock,
  zap: Zap,
  play: Play,
  flag: Flag,
  check: Check,
  git: GitBranch,
  cpu: Cpu,
  box: Box,
};

/** What a preset without an icon shows in a menu. */
export const SHAPE_ICON: Record<Shape, LucideIcon> = {
  box: Square,
  pill: RectangleHorizontal,
  diamond: Diamond,
  cylinder: Database,
  person: User,
  circle: Circle,
};

/** The four icons that keep their colour from the first component kinds. */
export const ICON_TONE: Partial<Record<IconName, string>> = {
  laptop: "app",
  server: "service",
  database: "data",
  cloud: "external",
};

export const label = (name: string) => name[0].toUpperCase() + name.slice(1);

const known = (name: unknown): name is IconName =>
  typeof name === "string" && Object.hasOwn(ICON_OF, name);

/** The lucide icon for a name from the file; an unknown name has none. */
export const iconFor = (name: unknown): LucideIcon | null =>
  known(name) ? ICON_OF[name] : null;

export const toneOf = (name: unknown): string | undefined =>
  known(name) ? ICON_TONE[name] : undefined;
