import { useEffect } from "react";
import { codePanels } from "../workspaceState.ts";
import type { Workspace } from "../types";

/** Global shortcuts. Everything but Cmd-B is scoped to the Code shell: in Agent
 * or Chat they only fire while a page is open, because that page is part of
 * Code. Note they DO act on the workspace panels beneath an open page - that
 * has always been so, and is a separate decision from this one. */
export function useKeyboardShortcuts({
  active,
  selected,
  dialogOpen,
  mode,
  section,
  openPanelPicker,
  closeDialog,
  setZoomed,
  setSelected,
  setSidebar,
  closePanel,
}: {
  active: Workspace;
  selected: string;
  dialogOpen: boolean;
  mode: string;
  section: boolean;
  openPanelPicker: () => void;
  closeDialog: () => void;
  setZoomed: (
    value: string | null | ((old: string | null) => string | null),
  ) => void;
  setSelected: (value: string) => void;
  setSidebar: (value: (old: boolean) => boolean) => void;
  closePanel: (panelId: string) => void;
}) {
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        closeDialog();
        setZoomed(null);
      }
      if (!event.metaKey) return;
      if (mode !== "Code" && !section && event.key !== "b") return;
      if (event.key === "k" || event.key === "t") {
        event.preventDefault();
        openPanelPicker();
      }
      if (event.key === "b") {
        event.preventDefault();
        setSidebar((value) => !value);
      }
      const currentPanel =
        active.panels.find((p) => p.id === selected) || active.panels[0];
      if (event.key === "Enter" && currentPanel) {
        event.preventDefault();
        setZoomed((value) => (value ? null : currentPanel.id));
      }
      if (event.key === "w" && !dialogOpen) {
        event.preventDefault();
        if (currentPanel) closePanel(currentPanel.id);
      }
      const panels = codePanels(active);
      if (/^[1-9]$/.test(event.key)) {
        const panel = panels[Number(event.key) - 1];
        if (panel) {
          event.preventDefault();
          setSelected(panel.id);
          setZoomed(null);
        }
      }
      if (
        event.shiftKey &&
        ["BracketLeft", "BracketRight"].includes(event.code) &&
        panels.length
      ) {
        event.preventDefault();
        const index = panels.findIndex((p) => p.id === selected);
        setSelected(
          panels[
            (Math.max(0, index) +
              (event.code === "BracketRight" ? 1 : -1) +
              panels.length) %
              panels.length
          ]?.id,
        );
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [
    selected,
    active,
    dialogOpen,
    mode,
    section,
    openPanelPicker,
    closeDialog,
    setZoomed,
    setSelected,
    setSidebar,
    closePanel,
  ]);
}
