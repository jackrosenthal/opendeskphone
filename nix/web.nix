# The web frontend: TypeScript, React, Tailwind, Vite.  Dependencies come
# from web/pnpm-lock.yaml.  The Connect client code generated from proto/
# is committed under web/src/gen, so nothing is generated or fetched during
# the build.  Output: the Vite `dist/` directory.
{
  lib,
  stdenv,
  nodejs,
  pnpm,
  pnpmConfigHook,
  fetchPnpmDeps,
  pnpmDepsHash,
}:
let
  root = ../.;
  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      (lib.fileset.fileFilter (f: f.name != "node_modules" && f.name != "dist") (root + "/web"))
    ];
  };
in
stdenv.mkDerivation (finalAttrs: {
  pname = "opendeskphone-web-frontend";
  version = "3.1.0";
  inherit src;
  sourceRoot = "source/web";
  nativeBuildInputs = [
    nodejs
    pnpm
    pnpmConfigHook
  ];
  pnpmDeps = fetchPnpmDeps {
    inherit (finalAttrs)
      pname
      version
      src
      sourceRoot
      ;
    fetcherVersion = 5;
    hash = pnpmDepsHash;
  };
  buildPhase = ''
    runHook preBuild
    pnpm run build
    runHook postBuild
  '';
  installPhase = ''
    runHook preInstall
    cp -r dist $out
    runHook postInstall
  '';
})
