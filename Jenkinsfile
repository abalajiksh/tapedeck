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
    DEPLOY_USER = credentials('tapedeck-deploy-user')
    
    // SonarQube configuration
    SONAR_PROJECT_KEY = 'tapedeck'
    SONAR_PROJECT_NAME = 'Tapedeck'
    SONAR_SOURCES = 'src'
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
            // Retrieve the scanner tool path explicitly
            def scannerHome = tool name: 'SonarQube Scanner', type: 'hudson.plugins.sonar.SonarRunnerInstallation'
            
            withSonarQubeEnv('SonarQube') {
              // Explicitly inject the token again to ensure authentication works
              withCredentials([string(credentialsId: 'sonarqube-token', variable: 'SONAR_TOKEN')]) {
                echo "=========================================="
                echo "Running SonarQube Analysis"
                echo "=========================================="
                
                // Use absolute path and explicit token
                sh """
                  "${scannerHome}/bin/sonar-scanner" \
                    -Dsonar.projectKey=${SONAR_PROJECT_KEY} \
                    -Dsonar.projectName="${SONAR_PROJECT_NAME}" \
                    -Dsonar.sources=${SONAR_SOURCES} \
                    -Dsonar.token=\$SONAR_TOKEN
                """
              }
            }
          } catch (Exception e) {
            echo "⚠ SonarQube analysis failed: ${e.message}"
            echo "  Verify: SonarQube server configured in Jenkins System settings"
            echo "  Verify: SonarQube Scanner tool installed in Global Tool Configuration"
          }
        }
      }
    }

    stage('Deploy') {
      steps {
        script {
          sshagent(credentials: ['tapedeck-ssh-key']) {
            // Use shell environment variables (prefixed with $) instead of Groovy interpolation
            sh """
              echo "=========================================="
              echo "Deploying to \$DEPLOY_HOST"
              echo "=========================================="
              
              # Test SSH connection
              echo "Testing SSH connection to \$DEPLOY_USER@\$DEPLOY_HOST..."
              ssh -o StrictHostKeyChecking=no "\$DEPLOY_USER@\$DEPLOY_HOST" 'echo "SSH connection successful"'
              
              # Copy binary to target server
              echo "Copying binary..."
              scp -o StrictHostKeyChecking=no target/release/tapedeck "\$DEPLOY_USER@\$DEPLOY_HOST:\$DEPLOY_DIR/"
              
              # Make executable and restart service
              echo "Restarting service..."
              ssh -o StrictHostKeyChecking=no "\$DEPLOY_USER@\$DEPLOY_HOST" \
                "chmod +x \$DEPLOY_DIR/tapedeck && \
                 systemctl restart tapedeck && \
                 systemctl status tapedeck --no-pager"
              
              echo "Deployment completed successfully!"
            """
          }
        }
      }
    }

    stage('Archive') {
      steps {
        script {
          def version = sh(
            script: "cargo pkgid | cut -d# -f2 | cut -d@ -f2",
            returnStdout: true
          ).trim()

          def commitSha = sh(
            script: 'git rev-parse --short HEAD',
            returnStdout: true
          ).trim()

          echo "Detected version: ${version}"
          echo "Commit SHA: ${commitSha}"

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
      echo 'Build and deployment completed successfully!'
    }
    failure {
      echo 'Build or deployment failed!'
    }
  }
}
