const fs = require("node:fs/promises");
const path = require("node:path");
const { createHash, randomUUID } = require("node:crypto");
const { HERDR_CONTRACT, releaseArtifact } = require("./herdr-contract.cjs");
const { HerdrError } = require("./herdr.cjs");
const { quote } = require("./connections.cjs");

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

async function installPinnedHerdr(
  directory,
  { platform = process.platform, arch = process.arch, fetchAsset = fetch } = {},
) {
  if (typeof directory !== "string" || !path.isAbsolute(directory))
    throw new Error("Invalid managed Herdr directory.");
  const artifact = releaseArtifact(platform, arch);
  const releaseDirectory = path.join(directory, HERDR_CONTRACT.version);
  const binary = path.join(releaseDirectory, "herdr");
  try {
    if (sha256(await fs.readFile(binary)) !== artifact.sha256)
      throw new HerdrError(
        "HERDR_CHECKSUM_MISMATCH",
        "The existing managed Herdr binary has a different checksum.",
      );
    return {
      binary,
      version: HERDR_CONTRACT.version,
      sha256: artifact.sha256,
      installed: false,
    };
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  const response = await fetchAsset(artifact.url, {
    signal: AbortSignal.timeout(30000),
  });
  if (!response.ok || !response.body)
    throw new Error(`Herdr download failed (${response.status}).`);
  const chunks = [];
  let size = 0;
  for await (const chunk of response.body) {
    size += chunk.length;
    if (size > 64 * 1024 * 1024)
      throw new Error("Herdr download exceeds 64 MB.");
    chunks.push(Buffer.from(chunk));
  }
  const bytes = Buffer.concat(chunks);
  if (sha256(bytes) !== artifact.sha256)
    throw new HerdrError(
      "HERDR_CHECKSUM_MISMATCH",
      "Herdr download checksum does not match the pinned release.",
    );
  await fs.mkdir(releaseDirectory, { recursive: true, mode: 0o700 });
  const temporary = path.join(releaseDirectory, `.herdr-${randomUUID()}`);
  try {
    await fs.writeFile(temporary, bytes, { flag: "wx", mode: 0o700 });
    try {
      await fs.link(temporary, binary);
    } catch (error) {
      if (
        error.code !== "EEXIST" ||
        sha256(await fs.readFile(binary)) !== artifact.sha256
      )
        throw error;
    }
  } finally {
    await fs.rm(temporary, { force: true });
  }
  return {
    binary,
    version: HERDR_CONTRACT.version,
    sha256: artifact.sha256,
    installed: true,
  };
}

async function installRemoteHerdr(endpoint, connections) {
  const source = `import hashlib,json,os,platform,tempfile,urllib.request
artifacts=json.loads(${JSON.stringify(JSON.stringify(HERDR_CONTRACT.artifacts))})
system={"Linux":"linux","Darwin":"darwin"}.get(platform.system(),platform.system())
machine={"x86_64":"x64","amd64":"x64","aarch64":"arm64","arm64":"arm64"}.get(platform.machine(),platform.machine())
asset=artifacts.get(system+"-"+machine)
if asset is None: raise RuntimeError("No verified Herdr release for this SSH host")
version=${JSON.stringify(HERDR_CONTRACT.version)}
directory=os.path.expanduser("~/.local/share/sushiai/herdr/"+version)
binary=os.path.join(directory,"herdr")
def verify(data):
 if hashlib.sha256(data).hexdigest()!=asset["sha256"]: raise RuntimeError("Herdr checksum mismatch")
installed=False
if os.path.exists(binary):
 with open(binary,"rb") as existing: verify(existing.read())
else:
 url="https://github.com/herdrdev/herdr/releases/download/v"+version+"/"+asset["name"]
 with urllib.request.urlopen(url,timeout=30) as response: data=response.read(64*1024*1024+1)
 if len(data)>64*1024*1024: raise RuntimeError("Herdr download exceeds 64 MB")
 verify(data)
 os.makedirs(directory,mode=0o700,exist_ok=True)
 descriptor,temporary=tempfile.mkstemp(prefix=".herdr-",dir=directory)
 try:
  with os.fdopen(descriptor,"wb") as output: output.write(data)
  os.chmod(temporary,0o700)
  try: os.link(temporary,binary)
  except FileExistsError:
   with open(binary,"rb") as existing: verify(existing.read())
 finally: os.unlink(temporary)
 installed=True
print(json.dumps({"binary":binary,"version":version,"sha256":asset["sha256"],"installed":installed}))`;
  return JSON.parse(
    await connections.exec(endpoint, `python3 -c ${quote(source)}`, {
      timeout: 40000,
    }),
  );
}

module.exports = { installPinnedHerdr, installRemoteHerdr };
