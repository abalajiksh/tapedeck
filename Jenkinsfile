pipeline {
  agent any

  parameters {
    string(
      name: 'COMMIT_ID',
      defaultValue: 'main',
      description: 'Git commit ID, branch name, or tag to build (default: main)'
    )
  }

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
    DEPLOY_HOST = credentials('tapedeck-lxc-ip')
    DEPLOY_DIR = credentials('tapedeck-lxc-dir')
    DEPLOY_CRED = credentials('tapedeck-lxc-cred')
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
        script {
          echo "Building commit/branch: ${params.COMMIT_ID}"
          checkout([
            $class: 'GitSCM',
            branches: [[name: "${params.COMMIT_ID}"]],
            userRemoteConfigs: scm.userRemoteConfigs
          ])
        }
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

    stage('Deploy') {
      steps {
        script {
          withCredentials([sshUserPrivateKey(credentialsId: 'tapedeck-ssh-key', keyFileVariable: 'SSH_KEY')]) {
            sh """
              # Set proper permissions on the key file
              chmod 600 \${SSH_KEY}
              
              # Copy binary to target server
              scp -i \${SSH_KEY} -o StrictHostKeyChecking=no \
                target/release/tapedeck \
                \${DEPLOY_CRED_USR}@\${DEPLOY_HOST}:\${DEPLOY_DIR}/
              
              # Make executable and restart service
              ssh -i \${SSH_KEY} -o StrictHostKeyChecking=no \
                \${DEPLOY_CRED_USR}@\${DEPLOY_HOST} \
                "chmod +x \${DEPLOY_DIR}/tapedeck && \
                 systemctl restart tapedeck"
            """
          }
        }
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

          // Get short commit SHA for artifact naming
          def commitSha = sh(
            script: 'git rev-parse --short HEAD',
            returnStdout: true
          ).trim()

          echo "Detected version: ${version}"
          echo "Commit SHA: ${commitSha}"

          // Copy and archive with version and commit info
          sh "cp target/release/tapedeck target/release/tapedeck-${version}-${commitSha}"
          archiveArtifacts artifacts: "target/release/tapedeck-${version}-${commitSha}", fingerprint: true
        }
      }
    }
  }

  post {
    always {
      echo "Build completed for commit: ${params.COMMIT_ID}"
    }
    success {
      echo 'Deployment completed successfully!'
    }
    failure {
      echo 'Deployment failed!'
    }
  }
}
