class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "1.3.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.3.0/r105-macos-arm64.tar.gz"
      sha256 "790ca154c341d1727ad99ee530a41d9e37f02759504fd6a6cfcedf6a85070436"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.3.0/r105-macos-x86_64.tar.gz"
      sha256 "f25002f206ec6b0a07b6137b89e0c025b05317c97018d124f6280751044ad5bd"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.3.0/r105-linux-aarch64.tar.gz"
      sha256 "b6db668b460f31b9ebd744f90b971fe5ce34b9571c810c3565ba4d2ec862c0fb"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.3.0/r105-linux-x86_64.tar.gz"
      sha256 "7e4a91de16373afe560fa18e845ba372f51c5f0c7dd8d2de04d6139389e5f516"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
