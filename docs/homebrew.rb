class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "1.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.1.0/r105-macos-arm64.tar.gz"
      sha256 "36070a28d113a2e007495d7d2233d2ce8232b9a1b7012fa9f573b607c7d7f102"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.1.0/r105-macos-x86_64.tar.gz"
      sha256 "d65ede4874f60b19bc84f99ad26a23671d8953f9e9b12f0e8644eff32dc58225"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.1.0/r105-linux-aarch64.tar.gz"
      sha256 "b50ddcc201e35fb80077360d9341e5895c9c25ffe9496b04d691f740bbf7d73b"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.1.0/r105-linux-x86_64.tar.gz"
      sha256 "c9dee6e2f45371c2d2f6488c267d0f851758ca73c2a257e13b5a4a21c1a918ad"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
