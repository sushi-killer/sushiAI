const { request } = require("./herdr.cjs");

async function closeHerdrPane(socketPath, paneId, requestFn = request) {
  const { snapshot } = await requestFn(socketPath, "session.snapshot");
  const pane = snapshot.panes.find((item) => item.pane_id === paneId);
  const owner = snapshot.workspaces.find(
    (item) => item.workspace_id === pane?.workspace_id,
  );
  const primary = owner?.worktree && !owner.worktree.is_linked_worktree;
  if (!primary) {
    try {
      return await requestFn(socketPath, "pane.close", { pane_id: paneId });
    } catch (error) {
      if (
        error.code !== "confirmation_required" ||
        error.message !== "closing this pane would close a worktree group"
      )
        throw error;
    }
  }
  // Herdr 0.8.2 closes linked sessions with the primary workspace. Moving
  // first also protects them if another pane ends after the snapshot.
  const destination = {
    pane_id: paneId,
    destination: { type: "new_workspace" },
    focus: false,
  };
  let { move_result: moved } = await requestFn(
    socketPath,
    "pane.move",
    destination,
  );
  if (!moved?.changed && moved?.reason === "zoomed_tab") {
    const { zoom } = await requestFn(socketPath, "pane.zoom", {
      pane_id: paneId,
      mode: "off",
    });
    if (zoom?.zoomed === false)
      ({ move_result: moved } = await requestFn(
        socketPath,
        "pane.move",
        destination,
      ));
  }
  if (!moved?.changed || !moved.pane?.pane_id)
    throw new Error(
      "Herdr could not detach the session from its worktree group.",
    );
  return requestFn(socketPath, "pane.close", { pane_id: moved.pane.pane_id });
}

module.exports = { closeHerdrPane };
