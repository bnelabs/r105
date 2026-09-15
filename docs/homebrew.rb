class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "2.0.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.0.0/r105-macos-arm64.tar.gz"
      sha256 "79640a9f9305acdb708066ff0e54d32768a1ee940e5e8caea8eedf98d5ef7cee"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.0.0/r105-macos-x86_64.tar.gz"
      sha256 "29cd864fc6fbf6391c72c6fcd2efab17af89f14c51e2ce49cbd9633282e7a2cf"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.0.0/r105-linux-aarch64.tar.gz"
      sha256 "1725b9891599b0a928ef6e0355d43d3c8edfd5dd3abea08a0b11abe60165123e"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.0.0/r105-linux-x86_64.tar.gz"
      sha256 "f5c95633ac5b3337b098e1f62af96b9584176f57e0aa922125eb38156b1bcb29"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
