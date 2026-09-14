class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "1.2.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.2.0/r105-macos-arm64.tar.gz"
      sha256 "4290b1720cefa63216930404ecea3dc19e41fcd27e91f83b016f74ec958d8b81"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.2.0/r105-macos-x86_64.tar.gz"
      sha256 "d29869b9bbfb20da461801df244277c417ef8bde8344bee21160c000c1604512"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.2.0/r105-linux-aarch64.tar.gz"
      sha256 "3d98874c1cf05c0cad20d944f9be3fd724496a620f9813d1bc5b0e6601fd13fe"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.2.0/r105-linux-x86_64.tar.gz"
      sha256 "f1cb42d7f6391d6b786095b9e7da3fad8ebecf88f38b27c9cb71749f7b3583a7"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
