class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "2.3.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.3.1/r105-macos-arm64.tar.gz"
      sha256 "02a8e49f7cb2e7dfe892abd943a49ea7b61a881ae605fe90d71a2bbd330d44b1"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.3.1/r105-macos-x86_64.tar.gz"
      sha256 "7c4a73fe81fedf443cb0caa40284dc8040034f1c0baf38649ca5950616de60bf"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.3.1/r105-linux-aarch64.tar.gz"
      sha256 "defd12b7b07b046f6eed03d6bafcbb3cded0e9b9f458c5f32a1c46b849a330a2"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.3.1/r105-linux-x86_64.tar.gz"
      sha256 "e1f10166e8668bb0e0f3ceb719395e8bc4a9c56166c3ea785c4fa0aeb058c5c8"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
