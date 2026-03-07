# frozen_string_literal: true

Gem::Specification.new do |spec|
  spec.name          = "eddie-jekyll"
  spec.version       = "0.2.3"
  spec.authors       = ["Jason Grey"]
  spec.summary       = "Installer for integrating Eddie into Jekyll sites"
  spec.description   = "Provides a CLI helper that runs Eddie's Jekyll installer script."
  spec.homepage      = "https://github.com/jt55401/eddie"
  spec.license       = "GPL-3.0-only"
  spec.required_ruby_version = Gem::Requirement.new(">= 3.0.0")

  spec.metadata["homepage_uri"] = spec.homepage
  spec.metadata["source_code_uri"] = "https://github.com/jt55401/eddie"

  spec.files = Dir.chdir(__dir__) do
    Dir["lib/**/*.rb", "exe/*", "scripts/install.sh", "assets/*", "README.md", "LICENSE.txt"]
  end

  spec.bindir = "exe"
  spec.executables = ["eddie-jekyll-install"]
  spec.require_paths = ["lib"]
  spec.add_runtime_dependency "jt55401-eddie-cli", "~> 0.2", ">= 0.2.3"
end
