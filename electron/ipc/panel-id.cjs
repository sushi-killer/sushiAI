const MAX_VIEW_ID_LENGTH = 4096;

function validatePanelId(value) {
  if (typeof value !== "string" || !value || value.length > MAX_VIEW_ID_LENGTH)
    throw new Error("Invalid panel ID.");
  return value;
}

module.exports = { MAX_VIEW_ID_LENGTH, validatePanelId };
