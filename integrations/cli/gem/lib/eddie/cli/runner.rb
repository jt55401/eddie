# frozen_string_literal: true

require "fileutils"
require "net/http"
require "uri"
require "tmpdir"

require_relative "version"

module Eddie
  module CLI
    module Runner
      module_function

      def resolve_asset
        platform = RUBY_PLATFORM

        return "eddie-linux-amd64" if platform.match?(/linux/) && platform.match?(/x86_64|amd64/)
        return "eddie-linux-arm64" if platform.match?(/linux/) && platform.match?(/aarch64|arm64/)
        return "eddie-darwin-amd64" if platform.match?(/darwin/) && platform.match?(/x86_64|amd64/)
        return "eddie-darwin-arm64" if platform.match?(/darwin/) && platform.match?(/arm64/)
        return "eddie-windows-amd64.exe" if platform.match?(/mingw|mswin/) && platform.match?(/x64|x86_64|amd64/)
        return "eddie-windows-arm64.exe" if platform.match?(/mingw|mswin/) && platform.match?(/arm64/)

        raise "Unsupported platform for Eddie CLI: #{platform}. No release asset mapping is configured."
      end

      def package_version
        ENV.fetch("EDDIE_CLI_VERSION", Eddie::CLI::VERSION)
      end

      def cache_root
        ENV.fetch("EDDIE_CLI_CACHE_DIR", File.join(Dir.home, ".cache", "eddie-cli"))
      end

      def ensure_binary
        version = package_version
        asset = resolve_asset
        bin_name = asset.end_with?(".exe") ? "eddie.exe" : "eddie"

        version_dir = File.join(cache_root, version)
        bin_path = File.join(version_dir, bin_name)

        if File.exist?(bin_path)
          FileUtils.chmod(0o755, bin_path)
          return bin_path
        end

        FileUtils.mkdir_p(version_dir)
        url = "https://github.com/jt55401/eddie/releases/download/v#{version}/#{asset}"
        warn "Downloading Eddie CLI #{version} (#{asset})..."

        Dir.mktmpdir("eddie-cli", version_dir) do |tmp_dir|
          tmp_path = File.join(tmp_dir, bin_name)
          download(url, tmp_path)
          FileUtils.chmod(0o755, tmp_path)
          FileUtils.mv(tmp_path, bin_path)
        end

        bin_path
      end

      def download(url, destination, redirects = 0)
        raise "Too many redirects while downloading #{url}" if redirects > 5

        uri = URI(url)
        Net::HTTP.start(uri.host, uri.port, use_ssl: uri.scheme == "https") do |http|
          request = Net::HTTP::Get.new(uri)
          request["User-Agent"] = "eddie-cli-rubygems"
          response = http.request(request)

          case response
          when Net::HTTPSuccess
            File.open(destination, "wb") { |file| file.write(response.body) }
          when Net::HTTPRedirection
            location = response["location"]
            raise "Redirect missing location header for #{url}" unless location

            next_url = URI.join(url, location).to_s
            download(next_url, destination, redirects + 1)
          else
            raise "Download failed (#{response.code}): #{url}"
          end
        end
      end
    end
  end
end
