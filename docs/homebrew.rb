class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "1.4.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.4.0/r105-macos-arm64.tar.gz"
      sha256 "48b42dca7a8a8d9f22560578058d2077c0c6bb2e1037593efc58278a5a8d81a6"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.4.0/r105-macos-x86_64.tar.gz"
      sha256 "cbb4813d89267c50a9b268fc4078bfee4e954636fdf0ffc774ba8ab7a0fef064"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.4.0/r105-linux-aarch64.tar.gz"
      sha256 "b554db94b8ff88da8ab9b02d879141ef7f7448375bd4b3c11973716373b9e317"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.4.0/r105-linux-x86_64.tar.gz"
      sha256 "e842b536dd627dd2643efa813ef89389a19af2e6461cb29d8209aa3d776504fb"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
