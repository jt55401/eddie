# frozen_string_literal: true

Gem::Specification.new do |spec|
  spec.name          = "eddie-cli"
  spec.version       = "0.2.2"
  spec.authors       = ["Jason Grey"]
  spec.summary       = "Cross-platform launcher for the Eddie CLI"
  spec.description   = "Provides an eddie executable that downloads and runs tagged Eddie release binaries."
  spec.homepage      = "https://github.com/jt55401/eddie"
  spec.license       = "GPL-3.0-only"
  spec.required_ruby_version = Gem::Requirement.new(">= 3.0.0")

  spec.metadata["homepage_uri"] = spec.homepage
  spec.metadata["source_code_uri"] = "https://github.com/jt55401/eddie"

  spec.files = Dir.chdir(__dir__) do
    Dir["lib/**/*.rb", "exe/*", "README.md", "LICENSE.txt"]
  end

  spec.bindir = "exe"
  spec.executables = ["eddie"]
  spec.require_paths = ["lib"]
end
