let
  lock = builtins.fromJSON (builtins.readFile ./flake.lock);
  compatibilityName = lock.nodes.root.inputs.flake-compat;
  compatibility = lock.nodes.${compatibilityName}.locked;
  compatibilityUrl = compatibility.url or "https://github.com/edolstra/flake-compat/archive/${compatibility.rev}.tar.gz";
  compatibilitySource = builtins.fetchTarball {
    url = compatibilityUrl;
    sha256 = compatibility.narHash;
  };
  legacyOutputs = (import compatibilitySource) { src = ./.; };
in
legacyOutputs.shellNix
