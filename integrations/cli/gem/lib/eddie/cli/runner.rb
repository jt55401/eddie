# frozen_string_literal: true

require "digest"
require "fileutils"
require "net/http"
require "uri"
require "tmpdir"

require_relative "version"

module Eddie
  module CLI
    module Runner
      module_function

      # Asset names match the eddie release matrix: eddie-<os>-<arch>[.exe].
      # Only the platforms release.yml actually builds are listed here.
      def resolve_asset
        platform = RUBY_PLATFORM

        return "eddie-linux-x86_64" if platform.match?(/linux/) && platform.match?(/x86_64|amd64/)
        return "eddie-linux-aarch64" if platform.match?(/linux/) && platform.match?(/aarch64|arm64/)
        return "eddie-macos-x86_64" if platform.match?(/darwin/) && platform.match?(/x86_64|amd64/)
        return "eddie-macos-aarch64" if platform.match?(/darwin/) && platform.match?(/arm64/)
        return "eddie-windows-x86_64.exe" if platform.match?(/mingw|mswin/) && platform.match?(/x64|x86_64|amd64/)

        raise "Unsupported platform for Eddie CLI: #{platform}. Eddie releases eddie-linux-x86_64, " \
              "eddie-linux-aarch64, eddie-macos-x86_64, eddie-macos-aarch64, and eddie-windows-x86_64.exe. " \
              "Build from source for other platforms."
      end

      def package_version
        ENV.fetch("EDDIE_CLI_VERSION", Eddie::CLI::VERSION)
      end

      # OS cache directory convention, overridable with EDDIE_CLI_CACHE_DIR.
      def cache_root
        return ENV["EDDIE_CLI_CACHE_DIR"] if ENV["EDDIE_CLI_CACHE_DIR"]

        case RUBY_PLATFORM
        when /darwin/
          File.join(Dir.home, "Library", "Caches", "eddie-cli")
        when /mingw|mswin/
          local_app_data = ENV["LOCALAPPDATA"] || File.join(Dir.home, "AppData", "Local")
          File.join(local_app_data, "eddie-cli", "Cache")
        else
          File.join(ENV["XDG_CACHE_HOME"] || File.join(Dir.home, ".cache"), "eddie-cli")
        end
      end

      # Parses `sha256sum * > SHA256SUMS` output into { filename => hex digest }.
      def parse_sha256sums(text)
        sums = {}
        text.each_line do |line|
          line = line.strip
          next if line.length < 66

          hash = line[0, 64].downcase
          next unless hash.match?(/\A[0-9a-f]{64}\z/)

          filename = line[64..].sub(/\A[\s*]+/, "").strip
          sums[filename] = hash unless filename.empty?
        end
        sums
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
        release_base = "https://github.com/jt55401/eddie/releases/download/v#{version}"
        asset_url = "#{release_base}/#{asset}"
        sums_url = "#{release_base}/SHA256SUMS"
        warn "Downloading Eddie CLI #{version} (#{asset})..."

        Dir.mktmpdir("eddie-cli", version_dir) do |tmp_dir|
          tmp_path = File.join(tmp_dir, bin_name)
          download(asset_url, tmp_path)

          sums_text = String.new
          download_text(sums_url, sums_text)
          expected = parse_sha256sums(sums_text)[asset]
          raise "SHA256SUMS for v#{version} has no entry for #{asset}." unless expected

          actual = Digest::SHA256.file(tmp_path).hexdigest
          if actual != expected
            raise "Checksum mismatch for #{asset}: expected #{expected}, got #{actual}. " \
                  "Refusing to install a corrupted or tampered binary."
          end

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

      # Fetches a small text file (SHA256SUMS) and appends its body into `text`.
      def download_text(url, text, redirects = 0)
        raise "Too many redirects while downloading #{url}" if redirects > 5

        uri = URI(url)
        Net::HTTP.start(uri.host, uri.port, use_ssl: uri.scheme == "https") do |http|
          request = Net::HTTP::Get.new(uri)
          request["User-Agent"] = "eddie-cli-rubygems"
          response = http.request(request)

          case response
          when Net::HTTPSuccess
            text << response.body
          when Net::HTTPRedirection
            location = response["location"]
            raise "Redirect missing location header for #{url}" unless location

            download_text(URI.join(url, location).to_s, text, redirects + 1)
          else
            raise "Download failed (#{response.code}): #{url}"
          end
        end
      end
    end
  end
end
