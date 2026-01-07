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
          // Extract version from Cargo.toml
          def version = sh(
            script: "cargo metadata --format-version=1 --no-deps | grep '\"version\":' | head -n1 | cut -d'\"' -f4",
            returnStdout: true
          ).trim()

          echo "Version: ${version}"

          // Copy binary with version in the name
          sh "cp target/release/tapedeck target/release/tapedeck-${version}"

          // Archive the versioned binary
          archiveArtifacts artifacts: "target/release/tapedeck-${version}", fingerprint: true, allowEmptyArchive: false
        }
      }
    }
  }
}
