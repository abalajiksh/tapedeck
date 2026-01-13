pipeline {
  agent any

  parameters {
    string(
      name: 'COMMIT_ID',
      defaultValue: 'main',
      description: 'Git commit ID, branch name, or tag to build (default: main)'
    )
    booleanParam(
      name: 'SKIP_SONARQUBE',
      defaultValue: false,
      description: 'Skip SonarQube analysis'
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
    
    // SonarQube configuration
    SONAR_PROJECT_KEY = 'tapedeck'
    SONAR_PROJECT_NAME = 'Tapedeck'
    SONAR_SOURCES = 'src'
    SONARQUBE_URL = "${env.SONARQUBE-LXC ?: 'http://localhost:9000'}"
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

    stage('SonarQube Analysis') {
      when {
        expression { return !params.SKIP_SONARQUBE }
      }
      steps {
        script {
          try {
            withCredentials([string(credentialsId: 'sonarqube-token', variable: 'SONAR_TOKEN')]) {
              sh '''
                if ! command -v sonar-scanner >/dev/null 2>&1; then
                  echo "⚠ sonar-scanner not found, skipping SonarQube analysis"
                  exit 0
                fi
                
                echo "=========================================="
                echo "Running SonarQube Analysis"
                echo "=========================================="
                
                sonar-scanner \
                  -Dsonar.projectKey=${SONAR_PROJECT_KEY} \
                  -Dsonar.projectName="${SONAR_PROJECT_NAME}" \
                  -Dsonar.sources=${SONAR_SOURCES} \
                  -Dsonar.host.url=${SONARQUBE_URL} \
                  -Dsonar.login=${SONAR_TOKEN}
              '''
            }
          } catch (Exception e) {
            echo "⚠ SonarQube credential not found (sonarqube-token), skipping analysis"
            echo "  To enable: Add 'sonarqube-token' credential in Jenkins"
          }
        }
      }
    }

    stage('Deploy') {
      steps {
        script {
          withCredentials([sshUserPrivateKey(credentialsId: 'tapedeck-ssh-key', keyFileVariable: 'SSH_KEY', usernameVariable: 'SSH_USER')]) {
            sh """
              # Set proper permissions on the key file
              chmod 600 \${SSH_KEY}
              
              # Check key format and convert if needed
              if head -n 1 \${SSH_KEY} | grep -q 'BEGIN OPENSSH PRIVATE KEY'; then
                echo "Key is in OpenSSH format, converting to PEM..."
                ssh-keygen -p -m PEM -f \${SSH_KEY} -N '' || true
              fi
              
              # Test SSH connection first
              echo "Testing SSH connection..."
              ssh -i \${SSH_KEY} -o StrictHostKeyChecking=no -o BatchMode=yes \
                \${DEPLOY_CRED_USR}@\${DEPLOY_HOST} 'echo "SSH connection successful"'
              
              # Copy binary to target server
              echo "Copying binary..."
              scp -i \${SSH_KEY} -o StrictHostKeyChecking=no \
                target/release/tapedeck \
                \${DEPLOY_CRED_USR}@\${DEPLOY_HOST}:\${DEPLOY_DIR}/
              
              # Make executable and restart service
              echo "Restarting service..."
              ssh -i \${SSH_KEY} -o StrictHostKeyChecking=no \
                \${DEPLOY_CRED_USR}@\${DEPLOY_HOST} \
                "chmod +x \${DEPLOY_DIR}/tapedeck && \
                 systemctl restart tapedeck && \
                 systemctl status tapedeck --no-pager"
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
