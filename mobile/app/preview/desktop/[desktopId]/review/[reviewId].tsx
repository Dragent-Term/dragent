import { PullRequestDetailScreen } from "../../../../../src/features/reviews/PullRequestDetailScreen";
import { usePreviewRouteParams } from "../../../../../src/preview/usePreviewRouteParams";
export default function ReviewRoute() {
  const { desktopId, reviewId } = usePreviewRouteParams();
  return <PullRequestDetailScreen key={reviewId} desktopId={desktopId} reviewId={reviewId} />;
}
