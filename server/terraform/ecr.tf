# Container registry for the server image — SHARED by both environments. Each
# env tracks its own moving tag (":production" / ":staging") plus per-commit
# ":<sha>" tags, so a staging push never moves production's image. CI pushes
# here; each env's EC2 host polls ECR and pulls its own tag.
resource "aws_ecr_repository" "server" {
  name = "${var.app_name}-server"
  # MUTABLE because the deploy strategy moves each env's tag on every push (the
  # EC2 redeploy poller pulls the moving tag). IMMUTABLE would let the FIRST push
  # succeed and every subsequent same-tag push fail. Each build is still also
  # pushed under its immutable `:<sha>` tag for rollback/audit.
  image_tag_mutability = "MUTABLE"
  image_scanning_configuration {
    scan_on_push = true # basic vulnerability scanning
  }
  encryption_configuration {
    encryption_type = "AES256"
  }
}

# Keep only the last 15 images to control storage cost.
resource "aws_ecr_lifecycle_policy" "server" {
  repository = aws_ecr_repository.server.name
  policy = jsonencode({
    rules = [{
      rulePriority = 1
      description  = "Keep last 15 images"
      selection    = { tagStatus = "any", countType = "imageCountMoreThan", countNumber = 15 }
      action       = { type = "expire" }
    }]
  })
}
