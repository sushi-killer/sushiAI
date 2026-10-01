/** The folder name a project gets under ~/sushiai on a host. */
function projectSlug(name) {
  return (
    String(name)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "project"
  );
}

/** The folder name of a project that exists: the one it was given when it was
 * made (a rename never moves a checkout), else, for a project from before
 * slugs were kept, the one its current name gives. */
function slugOf(project) {
  return project?.slug || projectSlug(project?.name);
}

/** `base`, or `base-2`, `base-3`... when another project already has it, so
 * two projects never share a checkout folder on a host. */
function uniqueSlug(base, projects) {
  const taken = new Set(Object.values(projects).map(slugOf));
  let slug = base;
  for (let n = 2; taken.has(slug); n += 1) slug = `${base}-${n}`;
  return slug;
}

module.exports = { projectSlug, slugOf, uniqueSlug };
