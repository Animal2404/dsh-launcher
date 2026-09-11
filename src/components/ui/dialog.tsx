// 弹窗尺寸策略（v0.7.1 修复）：
// 原实现基线类含 `sm:max-w-sm`，与调用方传入的 max-w-* 特异性相同，
// 而 Tailwind v4 把 `sm:` 媒体查询变体输出在基础工具类之后 → 调用方宽度被恒定覆盖，
// 四个管理面板在 ≥640px 视口下全部退化为 384px（详见 CHANGELOG v0.7.1）。
// 现改为：宽度完全交还调用方（DialogContent 不再声明任何 max-width），
// 仅保留一个不冲突的视口安全宽度；结构上拆为「可滚动主体 + sticky 头部」。
import * as React from "react"
import { Dialog as DialogPrimitive } from "@base-ui/react/dialog"

import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { XIcon } from "lucide-react"

function Dialog({ ...props }: DialogPrimitive.Root.Props) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />
}

function DialogTrigger({ ...props }: DialogPrimitive.Trigger.Props) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />
}

function DialogPortal({ ...props }: DialogPrimitive.Portal.Props) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />
}

function DialogClose({ ...props }: DialogPrimitive.Close.Props) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />
}

function DialogOverlay({
  className,
  ...props
}: DialogPrimitive.Backdrop.Props) {
  return (
    <DialogPrimitive.Backdrop
      data-slot="dialog-overlay"
      className={cn(
        // 遮罩：更深一档的压暗 + 模糊，配合统一的 200ms 缓动（见 index.css --motion-*）
        // z-[300]：必须高于全部应用外壳层级（侧栏 210 / 拖拽 handle 220 /
        // 右栏遮罩 245~270），否则窗口较窄时弹窗会被侧栏盖住（v0.7.1 修复）
        "fixed inset-0 isolate z-[300] bg-black/45 duration-200 supports-backdrop-filter:backdrop-blur-[3px] data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0",
        className
      )}
      {...props}
    />
  )
}

function DialogContent({
  className,
  children,
  showCloseButton = true,
  ...props
}: DialogPrimitive.Popup.Props & {
  showCloseButton?: boolean
}) {
  return (
    <DialogPortal>
      <DialogOverlay />
      <DialogPrimitive.Popup
        data-slot="dialog-content"
        className={cn(
          // 布局：固定居中 + 网格；宽度不在此声明（交还调用方，避免级联覆盖）
          // overflow-y-auto：主体已独立滚动时它不会产生滚动条；无 DialogBody 的
          // 确认类弹窗（内容较高）则由它兜底滚动，避免内容被裁掉
          // z-[310]：高于遮罩（300）与应用外壳全部层级，保证弹窗始终在最上层
          "dialog-content fixed top-1/2 left-1/2 z-[310] grid -translate-x-1/2 -translate-y-1/2 gap-0 overflow-x-hidden overflow-y-auto rounded-xl bg-popover p-0 text-sm text-popover-foreground shadow-2xl ring-1 ring-foreground/10 duration-200 outline-none data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95",
          className
        )}
        {...props}
      >
        {children}
        {showCloseButton && (
          <DialogPrimitive.Close
            data-slot="dialog-close"
            aria-label="关闭对话框"
            render={
              <Button
                variant="ghost"
                className="absolute top-2.5 right-2.5 z-10"
                size="icon-sm"
                aria-label="关闭对话框"
              />
            }
          >
            <XIcon
            />
            <span className="sr-only">关闭</span>
          </DialogPrimitive.Close>
        )}
      </DialogPrimitive.Popup>
    </DialogPortal>
  )
}

function DialogHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn(
        // sticky 头部：长内容滚动时标题常驻，右侧预留关闭按钮位
        "sticky top-0 z-[5] flex flex-col gap-2 border-b border-border/60 bg-popover/95 px-4 py-3 pr-10 backdrop-blur-sm supports-backdrop-filter:bg-popover/80",
        className
      )}
      {...props}
    />
  )
}

function DialogBody({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-body"
      className={cn(
        // 主体：自适应高度并独立滚动（长列表不把弹窗撑出视口）
        "min-h-0 overflow-y-auto overscroll-contain px-4 py-4",
        className
      )}
      {...props}
    />
  )
}

function DialogFooter({
  className,
  showCloseButton = false,
  children,
  ...props
}: React.ComponentProps<"div"> & {
  showCloseButton?: boolean
}) {
  return (
    <div
      data-slot="dialog-footer"
      className={cn(
        // 底部操作区：窄宽度自动换行堆叠，宽宽度右对齐
        "flex flex-col-reverse gap-2 border-t border-border/60 bg-muted/40 px-4 py-3 sm:flex-row sm:justify-end",
        className
      )}
      {...props}
    >
      {children}
      {showCloseButton && (
        <DialogPrimitive.Close render={<Button variant="outline" />}>
          关闭
        </DialogPrimitive.Close>
      )}
    </div>
  )
}

function DialogTitle({ className, ...props }: DialogPrimitive.Title.Props) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn(
        "font-heading text-base leading-none font-medium",
        className
      )}
      {...props}
    />
  )
}

function DialogDescription({
  className,
  ...props
}: DialogPrimitive.Description.Props) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn(
        "text-sm text-muted-foreground *:[a]:underline *:[a]:underline-offset-3 *:[a]:hover:text-foreground",
        className
      )}
      {...props}
    />
  )
}

export {
  Dialog,
  DialogBody,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
}
