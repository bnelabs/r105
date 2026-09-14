class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "1.0.2"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.0.2/r105-macos-arm64.tar.gz"
      sha256 "e685d9cc3d0e90bfcd16d7b3af1437ac73a6170c5ec3ee0e246a0ddfe796f8ce"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.0.2/r105-macos-x86_64.tar.gz"
      sha256 "7f17ca24dfeb925b6d35b4b12513ac9fa1f06d729e670572da42776f49e9280b"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.0.2/r105-linux-aarch64.tar.gz"
      sha256 "33380be97917961938416a9297048a765e99302e915bad7560023e1d97cf7336"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.0.2/r105-linux-x86_64.tar.gz"
      sha256 "f9e84f640a393edd48cf6438e99f5f86cc4872fb5f88849596a8c621a56927dd"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
