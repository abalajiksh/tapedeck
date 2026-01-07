pipeline {
  agent any

  options {
    timestamps()
    disableConcurrentBuilds()
  }

  triggers {
    pollSCM('H/2 * * * *')
  }

  environment {
    CARGO_TERM_COLOR = 'always'
    PATH = "$HOME/.cargo/bin:$PATH"
  }

  stages {
    stage('Check Rust') {
      steps {
        sh 'cargo --version'
        sh 'rustc --version'
      }
    }

    stage('Checkout') {
      steps {
        checkout scm
      }
    }

    stage('Clean') {
      steps {
        sh 'cargo clean'
      }
    }

    stage('Release') {
      steps {
        sh 'cargo build --release'
      }
    }

    stage('Archive') {
      steps {
        script {
          // cargo pkgid returns a URL-like string ending in #name:version
          // We use 'cut' to extract just the version part after the last colon
          def version = sh(
            script: "cargo pkgid | cut -d# -f2 | cut -d@ -f2", 
            returnStdout: true
          ).trim()

          echo "Detected version: ${version}"

          // Copy and archive
          sh "cp target/release/tapedeck target/release/tapedeck-${version}"
          archiveArtifacts artifacts: "target/release/tapedeck-${version}", fingerprint: true
        }
      }
    }
  }
}
