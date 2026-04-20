import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { Icon, type IconName } from "../../icons";
import { Overlay } from "./Overlay";

export interface PaletteCommand {
  id: string;
  label: string;
  hint?: string;
  icon?: IconName;
  group?: string;
  run: () => void;
}

interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  commands: PaletteCommand[];
  placeholder?: string;
  emptyMessage?: ReactNode;
}

function score(q: string, label: string): number {
  if (!q) return 1;
  const l = label.toLowerCase();
  const needle = q.toLowerCase();
  if (l.startsWith(needle)) return 3;
  if (l.includes(` ${needle}`)) return 2;
  if (l.includes(needle)) return 1;
  // fuzzy: all chars in order
  let i = 0;
  for (const ch of l) {
    if (ch === needle[i]) i++;
    if (i === needle.length) return 0.5;
  }
  return 0;
}

export function CommandPalette({
  open,
  onClose,
  commands,
  placeholder = "Type a command or jump to…",
  emptyMessage = "No matching commands.",
}: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (open) {
      setQuery("");
      setActive(0);
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  const filtered = useMemo(() => {
    const scored = commands
      .map((c) => ({ c, s: score(query, c.label) }))
      .filter((x) => x.s > 0)
      .sort((a, b) => b.s - a.s);
    return scored.map((x) => x.c);
  }, [query, commands]);

  useEffect(() => {
    if (active >= filtered.length) setActive(0);
  }, [filtered, active]);

  const run = useCallback(
    (cmd: PaletteCommand) => {
      onClose();
      // Defer so overlay unmount doesn't eat the click.
      queueMicrotask(() => cmd.run());
    },
    [onClose],
  );

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setActive((i) => Math.min(i + 1, filtered.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setActive((i) => Math.max(i - 1, 0));
      } else if (e.key === "Enter") {
        e.preventDefault();
        const cmd = filtered[active];
        if (cmd) run(cmd);
      }
    },
    [filtered, active, run],
  );

  const onQueryChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => setQuery(e.target.value),
    [],
  );

  return (
    <Overlay
      open={open}
      onClose={onClose}
      modal
      width={520}
      maxHeight="60vh"
      className="ui-palette"
      title={null}
    >
      <div className="ui-palette__inputwrap">
        <Icon name="command" size={14} />
        <input
          ref={inputRef}
          className="ui-palette__input"
          placeholder={placeholder}
          value={query}
          onChange={onQueryChange}
          onKeyDown={onKeyDown}
          spellCheck={false}
          autoComplete="off"
        />
      </div>
      <ul className="ui-palette__list" role="listbox">
        {filtered.length === 0 ? (
          <li className="ui-palette__empty">{emptyMessage}</li>
        ) : (
          filtered.map((cmd, i) => (
            <PaletteItem
              key={cmd.id}
              cmd={cmd}
              index={i}
              active={i === active}
              setActive={setActive}
              run={run}
            />
          ))
        )}
      </ul>
    </Overlay>
  );
}

function PaletteItem({
  cmd,
  index,
  active,
  setActive,
  run,
}: {
  cmd: PaletteCommand;
  index: number;
  active: boolean;
  setActive: (i: number) => void;
  run: (cmd: PaletteCommand) => void;
}) {
  const onMouseEnter = useCallback(
    () => setActive(index),
    [setActive, index],
  );
  const onClick = useCallback(() => run(cmd), [run, cmd]);
  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        run(cmd);
      }
    },
    [run, cmd],
  );
  return (
    <li
      role="option"
      aria-selected={active}
      className={"ui-palette__item" + (active ? " ui-palette__item--active" : "")}
      onMouseEnter={onMouseEnter}
      onClick={onClick}
      onKeyDown={onKeyDown}
    >
      <span className="ui-palette__icon">
        {cmd.icon ? <Icon name={cmd.icon} size={14} /> : null}
      </span>
      <span className="ui-palette__label">{cmd.label}</span>
      {cmd.group ? <span className="ui-palette__group">{cmd.group}</span> : null}
      {cmd.hint ? <span className="ui-palette__hint">{cmd.hint}</span> : null}
    </li>
  );
}
